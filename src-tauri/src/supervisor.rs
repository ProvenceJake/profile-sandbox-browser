use crate::store::atomic;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs, mem,
    path::Path,
    process::Command,
    sync::mpsc::Sender,
    thread,
    time::{Duration, Instant},
};

pub fn usage(pid: i32) -> Option<libc::rusage_info_v0> {
    let mut u: libc::rusage_info_v0 = unsafe { mem::zeroed() };
    let result =
        unsafe { libc::proc_pid_rusage(pid, 0, (&mut u as *mut libc::rusage_info_v0).cast()) };
    (result == 0).then_some(u)
}
pub fn register(dir: &Path, pid: u32, marker: &str, wake: Sender<()>) -> Result<()> {
    fs::create_dir_all(dir)?;
    let start = usage(pid as i32)
        .context("无法确认子进程身份")?
        .ri_proc_start_abstime;
    atomic(
        &dir.join(pid.to_string()),
        &serde_json::to_vec(&json!({"start":start,"marker":marker}))?,
    )?;
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return Ok(());
    }
    unsafe {
        libc::fcntl(queue, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let ev = libc::kevent {
        ident: pid as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    if unsafe { libc::kevent(queue, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
        unsafe { libc::close(queue) };
        let _ = wake.send(());
        return Ok(());
    }
    thread::spawn(move || {
        let mut out = unsafe { mem::zeroed() };
        unsafe {
            libc::kevent(queue, std::ptr::null(), 0, &mut out, 1, std::ptr::null());
            libc::close(queue);
        }
        let _ = wake.send(());
    });
    Ok(())
}
pub fn recover(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for f in fs::read_dir(dir)? {
        let f = f?;
        let Ok(pid) = f.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let record: Value = serde_json::from_slice(&fs::read(f.path())?)?;
        if pid > 1
            && usage(pid).is_some_and(|u| Some(u.ri_proc_start_abstime) == record["start"].as_u64())
        {
            let cmd = Command::new("/bin/ps")
                .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
                .output()?;
            if record["marker"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && String::from_utf8_lossy(&cmd.stdout).contains(s))
            {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
                for _ in 0..30 {
                    if !usage(pid)
                        .is_some_and(|u| Some(u.ri_proc_start_abstime) == record["start"].as_u64())
                    {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                if usage(pid)
                    .is_some_and(|u| Some(u.ri_proc_start_abstime) == record["start"].as_u64())
                {
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
        }
        fs::remove_file(f.path())?;
    }
    Ok(())
}
pub struct Sampler {
    last: Instant,
    times: HashMap<(i32, u64), u64>,
}
impl Sampler {
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            times: HashMap::new(),
        }
    }
    #[allow(deprecated)]
    pub fn sample(&mut self, pid: i32) -> (f64, f64) {
        let mut queue = vec![pid];
        let mut seen = HashSet::new();
        let mut memory = 0;
        let mut delta = 0;
        let mut next = HashMap::new();
        while let Some(p) = queue.pop() {
            if !seen.insert(p) {
                continue;
            }
            let Some(u) = usage(p) else { continue };
            let key = (p, u.ri_proc_start_abstime);
            let total = u.ri_user_time + u.ri_system_time;
            memory += u.ri_phys_footprint;
            if let Some(last) = self.times.get(&key) {
                delta += total.saturating_sub(*last);
            }
            next.insert(key, total);
            let mut children = [0i32; 256];
            let count = unsafe {
                libc::proc_listchildpids(
                    p,
                    children.as_mut_ptr().cast(),
                    mem::size_of_val(&children) as i32,
                )
            };
            if count > 0 {
                queue.extend(
                    children
                        .iter()
                        .take((count as usize).min(children.len()))
                        .copied()
                        .filter(|p| *p > 1),
                );
            }
        }
        let mut scale = libc::mach_timebase_info { numer: 1, denom: 1 };
        #[allow(deprecated)]
        unsafe {
            libc::mach_timebase_info(&mut scale);
        }
        let pct = delta as f64 * scale.numer as f64
            / scale.denom as f64
            / 1e9
            / self.last.elapsed().as_secs_f64().max(0.001)
            * 100.;
        self.times = next;
        self.last = Instant::now();
        (memory as f64 / 1048576., pct)
    }
}
