//! Slint 技术验证入口。

slint::include_modules!();

use std::path::PathBuf;
use std::time::Instant;

const SETTLE_SECONDS: u64 = 6;

#[cfg(target_os = "windows")]
fn sample_ws_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut info = std::mem::MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let handle = unsafe { GetCurrentProcess() };
    let ok = unsafe { GetProcessMemoryInfo(handle, info.as_mut_ptr(), size) };
    if ok == 0 {
        return 0;
    }
    unsafe { info.assume_init().WorkingSetSize as u64 }
}

#[cfg(target_os = "linux")]
fn sample_ws_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let r = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if r != 0 {
        return 0;
    }
    unsafe { (usage.assume_init().ru_maxrss as u64) * 1024 }
}

#[cfg(target_os = "macos")]
fn sample_ws_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let r = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if r != 0 {
        return 0;
    }
    unsafe { usage.assume_init().ru_maxrss as u64 }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn sample_ws_bytes() -> u64 {
    0
}

fn fmt_mb(n: u64) -> String {
    format!("{:.1} MB", (n as f64) / (1024.0 * 1024.0))
}

fn log_path() -> PathBuf {
    std::env::temp_dir().join("ferric-spike").join("spike.log")
}

fn append_log(line: &str) {
    let p = log_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let _ = writeln!(f, "[{ts}] {line}");
    }
}

fn main() -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .renderer_name("software".into())
        .select()?;

    let app = App::new()?;
    let started = Instant::now();
    append_log("spike started (slint-spike v0.1.0, software renderer)");

    {
        let app_handle = app.as_weak();
        app.on_refresh_rss(move || {
            let ws = sample_ws_bytes();
            let elapsed = started.elapsed().as_secs_f64();
            append_log(&format!("sample t={elapsed:.1}s ws_bytes={ws}"));
            if let Some(ui) = app_handle.upgrade() {
                let txt: slint::SharedString = if ws == 0 {
                    "采样失败".into()
                } else {
                    fmt_mb(ws).into()
                };
                ui.set_rss_text(txt);
            }
        });
    }

    {
        let app_handle = app.as_weak();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(SETTLE_SECONDS));
            let ws = sample_ws_bytes();
            let elapsed = started.elapsed().as_secs_f64();
            append_log(&format!("settle t={elapsed:.1}s ws_bytes={ws}"));
            if let Some(ui) = app_handle.upgrade() {
                let txt: slint::SharedString = if ws == 0 {
                    "采样失败".into()
                } else {
                    fmt_mb(ws).into()
                };
                ui.set_rss_text(txt);
            }
        });
    }

    app.run()
}