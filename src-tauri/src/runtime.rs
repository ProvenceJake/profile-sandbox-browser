use crate::store::{checked_url, Environment, Store};
use anyhow::{anyhow, ensure, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::LazyLock,
    time::Duration,
};

pub const TRACE: &str = "https://www.cloudflare.com/cdn-cgi/trace";
pub const IPIFY: &str = "https://api64.ipify.org";
pub const CHROME_VERSION: &str = "158.0.8127.36";
const SYSTEM_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const CHROME_REL: &str = "Google Chrome.app/Contents/MacOS/Google Chrome";
pub fn chrome(store: &Store) -> PathBuf {
    let installed = store
        .root
        .join("runtime")
        .join(CHROME_VERSION)
        .join(CHROME_REL);
    if installed.is_file()
        && (!Path::new(SYSTEM_CHROME).is_file() || store.setting("browser", "system") == "managed")
    {
        installed
    } else {
        SYSTEM_CHROME.into()
    }
}
pub fn engine() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let bundled = exe.parent().context("运行时目录无效")?.join("mihomo");
    if bundled.is_file() {
        return Ok(bundled);
    }

    anyhow::bail!("应用包缺少 Mihomo，请重新安装")
}
fn fetch(url: &str, digest: &str, dir: &Path, max: u64) -> Result<tempfile::NamedTempFile> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let mut reader = client.get(url).send()?.error_for_status()?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 65536];
    let mut count = 0;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        ensure!(count <= max, "下载超过预期限额");
        hasher.update(&buffer[..n]);
        tmp.write_all(&buffer[..n])?;
    }
    ensure!(
        format!("{:x}", hasher.finalize()) == digest,
        "下载文件校验失败"
    );
    tmp.flush()?;
    Ok(tmp)
}
pub fn install_geo(root: &Path) -> Result<()> {
    let dir = root.join("runtime");
    let archive = fetch(
        "https://download.db-ip.com/free/dbip-city-lite-2026-11.mmdb.gz",
        "b69c625515f636ac92112b71df26bb92a5748f648c79bad124ecd9ba1c97bf4a",
        &dir,
        160_000_000,
    )?;
    let mut target = tempfile::NamedTempFile::new_in(&dir)?;
    io::copy(
        &mut flate2::read::GzDecoder::new(File::open(archive.path())?),
        &mut target,
    )?;
    maxminddb::Reader::open_mmap(target.path())?;
    target.persist(dir.join("city.mmdb"))?;
    Ok(())
}
pub fn install_chrome(root: &Path) -> Result<()> {
    let dir = root.join("runtime");
    let dest = dir.join(CHROME_VERSION);
    if dest.join(CHROME_REL).is_file() {
        verify_chrome(&dest.join(CHROME_REL))?;
        return Ok(());
    }
    let archive=fetch("https://dl.google.com/release2/chrome/babdejpqcsbnf7kpjv4mwnk4gu_158.0.8127.36/GoogleChrome-158.0.8127.36.dmg",
        "6532fcf302bd6a76d83ed2f6c91f46abab142800e6f1073a093f575a0418d47f",&dir,450_000_000)?;
    install_image(archive.path(), &dir, &dest)
}
fn install_image(archive: &Path, dir: &Path, dest: &Path) -> Result<()> {
    let stage = tempfile::tempdir_in(dir)?;
    let mount = tempfile::tempdir_in(dir)?;
    ensure!(
        Command::new("/usr/bin/hdiutil")
            .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
            .arg(mount.path())
            .arg(archive)
            .output()?
            .status
            .success(),
        "无法挂载 Chrome 安装映像"
    );
    let copied = Command::new("/usr/bin/ditto")
        .args(["--norsrc", "--noextattr"])
        .arg(mount.path().join("Google Chrome.app"))
        .arg(stage.path().join("Google Chrome.app"))
        .output();
    let detached = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(mount.path())
        .output()?;
    if !detached.status.success() {
        Command::new("/usr/bin/hdiutil")
            .args(["detach", "-force"])
            .arg(mount.path())
            .output()?
            .status
            .success()
            .then_some(())
            .context("只读安装映像未能卸载")?;
    }
    ensure!(copied?.status.success(), "复制 Chrome 失败");
    ensure!(
        verify_chrome(&stage.path().join(CHROME_REL))? == CHROME_VERSION,
        "浏览器版本与签名清单不一致"
    );
    fs::rename(stage.path(), dest)?;
    Ok(())
}
pub fn verify_chrome(binary: &Path) -> Result<String> {
    let app = binary
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .context("Chrome 应用包缺失")?;
    ensure!(
        Command::new("/usr/bin/codesign")
            .args([
                "--verify",
                "--strict",
                "-R",
                "=anchor apple generic and certificate leaf[subject.OU] = EQHXZ8M8AV"
            ])
            .arg(app)
            .output()?
            .status
            .success(),
        "Chrome 签名验证失败"
    );
    let output = Command::new(binary).arg("--version").output()?;
    ensure!(output.status.success(), "读取 Chrome 版本失败");
    Ok(String::from_utf8(output.stdout)?
        .split_whitespace()
        .last()
        .context("缺少版本号")?
        .to_owned())
}
pub fn version(v: &str) -> Result<Vec<u32>> {
    v.split('.').map(|s| Ok(s.parse()?)).collect()
}
pub fn extract(raw: &str) -> Result<(IpAddr, String)> {
    let trimmed = raw.trim();
    let obj: Value = serde_json::from_str(trimmed).unwrap_or_default();
    let addr = raw
        .lines()
        .find_map(|s| s.strip_prefix("ip="))
        .or_else(|| obj.get("ip").and_then(Value::as_str))
        .unwrap_or(trimmed)
        .trim()
        .parse::<IpAddr>()
        .context("回显未返回有效 IP")?;
    let cc = raw
        .lines()
        .find_map(|s| s.strip_prefix("loc="))
        .or_else(|| obj["country"].as_str())
        .unwrap_or("")
        .trim();
    let public = match addr {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !v.is_private()
                && !v.is_loopback()
                && !v.is_link_local()
                && !v.is_documentation()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && matches!(b, 18 | 19))
                && !(a == 192 && b == 0 && c == 0)
        }
        IpAddr::V6(v) => {
            let h = v.segments();
            h[0] & 0xe000 == 0x2000 && !(h[0] == 0x2001 && h[1] == 0xdb8)
        }
    };
    ensure!(public, "回显未返回公网地址");
    Ok((addr, cc.to_owned()))
}
pub fn verify_exit(store: &Store, port: u16) -> Result<(IpAddr, String, String)> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))?)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    for url in [
        store.setting("echo", TRACE),
        store.setting("fallback", IPIFY),
    ] {
        if url.is_empty() {
            continue;
        }
        checked_url(&url)?;
        if let Ok(resp) = client.get(&url).send().and_then(|r| r.error_for_status()) {
            let mut body = String::new();
            if resp.take(16384).read_to_string(&mut body).is_ok() {
                if let Ok((ip, cc)) = extract(&body) {
                    return Ok((ip, cc, url));
                }
            }
        }
    }
    Err(anyhow!("节点出口校验失败，已阻止浏览器出网"))
}
pub fn locate(store: &Store, ip: IpAddr) -> Result<(Environment, String)> {
    let db = maxminddb::Reader::open_mmap(store.root.join("runtime/city.mmdb"))
        .context("请先在运行时页面下载离线城市库，或选择手动环境")?;
    let row: Value = db
        .lookup(ip)?
        .context("离线库未收录该地址，请使用手动环境")?;
    let country = row["country"]["iso_code"]
        .as_str()
        .context("缺少出口国家")?;
    let lat = row["location"]["latitude"]
        .as_f64()
        .context("缺少出口纬度")?;
    let lng = row["location"]["longitude"]
        .as_f64()
        .context("缺少出口经度")?;
    static ZONES: LazyLock<tzf_rs::EmbeddedFinder> = LazyLock::new(tzf_rs::EmbeddedFinder::new);
    static LANGS: LazyLock<Value> = LazyLock::new(|| {
        serde_json::from_str(include_str!("../../resources/locales.json")).unwrap()
    });
    let zone = ZONES.get_tz_name(lng, lat);
    let locale = LANGS[country]
        .as_str()
        .context("无法推导当地语言，请使用手动环境")?;
    let env = Environment {
        zone: zone.into(),
        locale: locale.into(),
        lat,
        lng,
    };
    env.validate()?;
    Ok((env, country.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the pinned Google Chrome DMG"]
    fn signed_install_image() -> Result<()> {
        let image = std::env::var("HARBOR_TEST_DMG")?;
        let stage = tempfile::tempdir()?;
        let target = stage.path().join(CHROME_VERSION);
        install_image(Path::new(&image), stage.path(), &target)?;
        assert_eq!(verify_chrome(&target.join(CHROME_REL))?, CHROME_VERSION);
        Ok(())
    }
}
