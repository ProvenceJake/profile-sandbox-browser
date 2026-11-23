use anyhow::{anyhow, bail, ensure, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn id() -> String {
    Uuid::new_v4().to_string()
}
pub fn valid_id(s: &str) -> Result<&str> {
    Uuid::parse_str(s)?;
    Ok(s)
}
pub fn name(s: &str) -> Result<String> {
    let value = s.trim();
    ensure!(
        !value.is_empty() && value.chars().count() <= 100 && !value.chars().any(char::is_control),
        "名称应为 1–100 个可显示字符"
    );
    Ok(value.into())
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("缺少数据目录")?)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[derive(Clone, Deserialize, Serialize, Debug)]
pub struct Environment {
    pub zone: String,
    pub locale: String,
    pub lat: f64,
    pub lng: f64,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            zone: "UTC".into(),
            locale: "en-US".into(),
            lat: 0.,
            lng: 0.,
        }
    }
}
impl Environment {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.zone.starts_with('/')
                && !self.zone.contains("..")
                && !self.zone.is_empty()
                && Path::new("/usr/share/zoneinfo").join(&self.zone).is_file(),
            "无效的 IANA 时区"
        );
        use std::io::Read;
        let mut magic = [0; 4];
        fs::File::open(Path::new("/usr/share/zoneinfo").join(&self.zone))?
            .read_exact(&mut magic)?;
        ensure!(&magic == b"TZif", "无效的 IANA 时区");
        ensure!(
            self.locale.len() <= 35
                && self.locale.split('-').all(|s| !s.is_empty()
                    && s.len() <= 8
                    && s.bytes().all(|c| c.is_ascii_alphanumeric())),
            "语言标签无效"
        );
        ensure!(
            (-90.0..=90.).contains(&self.lat) && (-180.0..=180.).contains(&self.lng),
            "坐标超出范围"
        );
        Ok(())
    }
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub node: String,
    pub auto: bool,
    #[serde(default)]
    pub env: Environment,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub used: i64,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub remote: bool,
    pub updated: i64,
    #[serde(skip_serializing, default)]
    pub secret: String,
}
pub struct Store {
    pub root: PathBuf,
    pub db: Connection,
}
const SERVICE: &str = "dev.provencejake.profileharbor.sources";
fn secret_get(key: &str) -> Result<Value> {
    let bytes = security_framework::passwords::get_generic_password(SERVICE, key)
        .context("无法读取系统钥匙串中的节点凭据")?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub fn checked_url(s: &str) -> Result<url::Url> {
    let u = url::Url::parse(s).map_err(|_| anyhow!("URL 格式无效"))?;
    ensure!(
        u.scheme() == "https"
            || (u.scheme() == "http"
                && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
        "需要 HTTPS URL（本机回环可用 HTTP）"
    );
    Ok(u)
}
pub fn fetch_source(url: &str) -> Result<String> {
    use std::io::Read;
    let u = checked_url(url)?;
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get(u)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| anyhow!("下载订阅失败，请检查地址或网络"))?;
    let mut body = String::new();
    response.take(8_388_609).read_to_string(&mut body)?;
    ensure!(body.len() <= 8_388_608, "订阅超过 8 MB");
    Ok(body)
}
impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        for folder in ["profiles", "trash", "runtime", "engine"] {
            fs::create_dir_all(root.join(folder))?;
        }
        let db = Connection::open(root.join("harbor.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS profiles(id TEXT PRIMARY KEY, data TEXT NOT NULL, deleted INTEGER);
        CREATE TABLE IF NOT EXISTS sources(id TEXT PRIMARY KEY,name TEXT NOT NULL,remote INTEGER NOT NULL,updated INTEGER NOT NULL,secret TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS nodes(id TEXT PRIMARY KEY,source TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,name TEXT NOT NULL,kind TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS launches(id TEXT PRIMARY KEY,profile TEXT NOT NULL,started INTEGER NOT NULL,ready_ms INTEGER,verify_ms INTEGER,ip TEXT,country TEXT,slot INTEGER,end_reason TEXT);
        CREATE TABLE IF NOT EXISTS metrics(launch TEXT NOT NULL,ts INTEGER NOT NULL,memory_mb REAL,cpu REAL,tabs INTEGER);
        CREATE INDEX IF NOT EXISTS metrics_ts ON metrics(ts);
        CREATE TABLE IF NOT EXISTS events(ts INTEGER NOT NULL,profile TEXT,detail TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS obsolete_keys(key TEXT PRIMARY KEY);
        PRAGMA user_version=1;")?;
        let s = Self { root, db };
        s.maintenance()?;
        Ok(s)
    }
    pub fn maintenance(&self) -> Result<()> {
        self.db
            .execute("DELETE FROM metrics WHERE ts<?", [now() - 30 * 86400])?;
        self.db
            .execute("DELETE FROM events WHERE ts<?", [now() - 30 * 86400])?;
        let mut stmt = self
            .db
            .prepare("SELECT id FROM profiles WHERE deleted IS NOT NULL")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let key = row?;
            valid_id(&key)?;
            let old = self.root.join("profiles").join(&key);
            let trash = self.root.join("trash").join(&key);
            if old.exists() && !trash.exists() {
                fs::rename(old, &trash)?;
            }
            let expired: bool = self.db.query_row(
                "SELECT deleted < ? FROM profiles WHERE id=?",
                params![now() - 7 * 86400, key],
                |r| r.get(0),
            )?;
            if expired {
                if trash.exists() {
                    fs::remove_dir_all(trash)?
                }
                self.db.execute("DELETE FROM profiles WHERE id=?", [key])?;
            }
        }
        let mut stmt = self.db.prepare("SELECT key FROM obsolete_keys")?;
        for key in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let key = key?;
            let result = security_framework::passwords::delete_generic_password(SERVICE, &key);
            if result.is_ok() || result.err().is_some_and(|e| e.code() == -25300) {
                self.db
                    .execute("DELETE FROM obsolete_keys WHERE key=?", [key])?;
            }
        }
        Ok(())
    }
    pub fn profile(&self, k: &str) -> Result<Profile> {
        valid_id(k)?;
        let data: String = self
            .db
            .query_row(
                "SELECT data FROM profiles WHERE id=? AND deleted IS NULL",
                [k],
                |r| r.get(0),
            )
            .context("找不到 Profile")?;
        Ok(serde_json::from_str(&data)?)
    }
    pub fn save(&self, p: &Profile) -> Result<()> {
        valid_id(&p.id)?;
        self.db.execute("INSERT INTO profiles(id,data) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![p.id,serde_json::to_string(p)?])?;
        Ok(())
    }
    pub fn profiles(&self) -> Result<Vec<Profile>> {
        let mut q = self
            .db
            .prepare("SELECT data FROM profiles WHERE deleted IS NULL ORDER BY rowid")?;
        let mut rows = q.query([])?;
        let mut v = vec![];
        while let Some(row) = rows.next()? {
            v.push(serde_json::from_str(&row.get::<_, String>(0)?)?)
        }
        Ok(v)
    }
    pub fn sources(&self) -> Result<Vec<Source>> {
        let mut q = self
            .db
            .prepare("SELECT id,name,remote,updated,secret FROM sources ORDER BY rowid")?;
        let rows = q.query_map([], |r| {
            Ok(Source {
                id: r.get(0)?,
                name: r.get(1)?,
                remote: r.get(2)?,
                updated: r.get(3)?,
                secret: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn nodes(&self) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT id,source,name,kind FROM nodes ORDER BY name")?;
        let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"source":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?,"kind":r.get::<_,String>(3)?})))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn has_node(&self, k: &str) -> Result<bool> {
        Ok(self
            .db
            .query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=?)", [k], |r| {
                r.get(0)
            })?)
    }
    pub fn provider(&self) -> Result<Value> {
        let mut nodes = vec![];
        for s in self.sources()? {
            let data = secret_get(&s.secret)?;
            nodes.extend(
                data["proxies"]
                    .as_array()
                    .context("节点数据损坏")?
                    .iter()
                    .cloned(),
            )
        }
        Ok(json!({"proxies":nodes}))
    }
    pub fn refresh(&mut self, k: &str) -> Result<()> {
        let source = self
            .sources()?
            .into_iter()
            .find(|s| s.id == k)
            .context("订阅不存在")?;
        let raw = secret_get(&source.secret)?;
        let url = raw["url"].as_str().context("此来源为本地导入")?;
        let body = fetch_source(url)?;
        self.import(k, &source.name, Some(url), &body)
    }
    pub fn import(&mut self, k: &str, title: &str, url: Option<&str>, body: &str) -> Result<()> {
        valid_id(k)?;
        let title = name(title)?;
        ensure!(body.len() <= 8_388_608, "节点数据过大");
        let doc: Value = serde_yaml::from_str(body).map_err(|_| anyhow!("YAML 格式无效"))?;
        let raw = if let Some(a) = doc["proxies"].as_array() {
            a.clone()
        } else if doc.is_object() && doc["type"].is_string() {
            vec![doc]
        } else {
            bail!("需要包含 proxies 列表的 Clash YAML 或单个节点")
        };
        ensure!(
            !raw.is_empty() && raw.len() <= 5000,
            "每个来源应有 1–5000 个节点"
        );
        let mut seen = HashSet::new();
        let mut summaries = vec![];
        let mut configs = vec![];
        for mut node in raw {
            let title = name(node["name"].as_str().context("缺少节点名称")?)?;
            ensure!(seen.insert(title.clone()), "来源内有重名节点");
            let kind = node["type"].as_str().context("缺少节点类型")?.to_owned();
            ensure!(
                matches!(
                    kind.as_str(),
                    "http"
                        | "socks5"
                        | "ss"
                        | "ssr"
                        | "vmess"
                        | "vless"
                        | "trojan"
                        | "hysteria"
                        | "hysteria2"
                        | "tuic"
                        | "anytls"
                        | "snell"
                        | "mieru"
                        | "wireguard"
                ),
                "不支持的节点协议"
            );
            ensure!(
                node["server"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
                    && node["port"]
                        .as_u64()
                        .is_some_and(|p| (1..=65535).contains(&p)),
                "节点缺少有效服务器或端口"
            );
            ensure!(
                node.get("dialer-proxy").is_none() && node.get("interface-name").is_none(),
                "不支持引用外部代理或指定网卡的节点"
            );
            let key = Uuid::new_v5(&Uuid::parse_str(k)?, title.as_bytes()).to_string();
            node["name"] = json!(key);
            summaries.push((key, title, kind));
            configs.push(node);
        }
        let secret = id();
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO obsolete_keys SELECT secret FROM sources WHERE id=?",
            [k],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO sources VALUES(?,?,?,?,?)",
            params![k, title, url.is_some(), now(), secret],
        )?;
        for (key, label, kind) in summaries {
            tx.execute(
                "INSERT INTO nodes VALUES(?,?,?,?)",
                params![key, k, label, kind],
            )?;
        }
        security_framework::passwords::set_generic_password(
            SERVICE,
            &secret,
            &serde_json::to_vec(&json!({"url":url,"proxies":configs}))?,
        )
        .context("凭据写入钥匙串失败")?;
        if let Err(e) = tx.commit() {
            let _ = security_framework::passwords::delete_generic_password(SERVICE, &secret);
            return Err(e.into());
        }
        self.maintenance()
    }
    pub fn remove_source(&mut self, k: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO obsolete_keys SELECT secret FROM sources WHERE id=?",
            [k],
        )?;
        tx.execute("DELETE FROM sources WHERE id=?", [k])?;
        tx.commit()?;
        self.maintenance()
    }
    pub fn event(&self, id: &str, text: &str) {
        let _ = self
            .db
            .execute("INSERT INTO events VALUES(?,?,?)", params![now(), id, text]);
    }
    pub fn setting(&self, k: &str, default: &str) -> String {
        self.db
            .query_row("SELECT value FROM settings WHERE key=?", [k], |r| r.get(0))
            .unwrap_or_else(|_| default.into())
    }
    pub fn setting_set(&self, k: &str, v: &str) -> Result<()> {
        self.db
            .execute("INSERT OR REPLACE INTO settings VALUES(?,?)", [k, v])?;
        Ok(())
    }
    pub fn records(&self, sql: &str) -> Result<Vec<Value>> {
        self.records_with(sql, [])
    }
    pub fn records_with(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Value>> {
        use rusqlite::types::ValueRef;
        let mut stmt = self.db.prepare(sql)?;
        let names = stmt
            .column_names()
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>();
        let mut rows = stmt.query(args)?;
        let mut out = vec![];
        while let Some(r) = rows.next()? {
            let mut obj = serde_json::Map::new();
            for (i, k) in names.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(f) => json!(f),
                    _ => Value::Null,
                };
                obj.insert(k.clone(), v);
            }
            out.push(Value::Object(obj));
        }
        Ok(out)
    }
}
