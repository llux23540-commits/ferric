//! 运行时内存采样：按需把 30 秒的进程内存采样写入 `memory.log`。
//!
//! # 用途
//!
//! 排查"为什么内存涨"或"为什么会卡"：对照运行期采样的进程工作集、Rust 堆内
//! 子系统（草稿、持久化、字体源字节）随时间的演变。这一份是**采样器**，不
//! 做归因、不做优化 —— 拿到数字再看图说话。
//!
//! # 设计原则
//!
//! - **按需触发**：默认不工作；只有设置里点"记录 30 秒内存"才开 30 秒采样，
//!   结束后写文件，UI 不再做任何事。绝不在主路径上常驻定时器。
//! - **追加写**：多次"记录"按时间顺序串在同一个 `memory.log` 末尾，方便对比
//!   多次会话（升级前/后、加载大文件前/后）的趋势。文件大小天然封顶：
//!   31 行 × ~80B ≈ 2.5KB / 次。
//! - **诚实字段**：egui 的字体 atlas 是 `epaint::TexturesManager` 私有字段，
//!   公开 API 拿不到总字节。本模块只报告**字体源字节**（内嵌 + CJK 加载字节），
//!   不假装能拿到 atlas 字节。详见 [`MemSnapshot::fonts_src_bytes`]。
//! - **失败静默**：OS 层 API 失败（psapi 在极少数容器里不可用、/proc 不存在
//!   等）只让那一帧的字段为 0，**不让采样器自己变成打不开应用的理由**。
//!
//! # 字段语义（与任务管理器对位）
//!
//! | 字段 | 来源 | 与任务管理器的对应 |
//! |---|---|---|
//! | `ws_bytes` | Windows: `GetProcessMemoryInfo` 的 `WorkingSetSize`；Unix: `getrusage(RUSAGE_SELF).ru_maxrss` 按平台换算 | 私有工作集 |
//! | `rss_bytes` | Windows: `GetProcessMemoryInfo` 的 `PagefileUsage`；Unix: 同上退化为 0（API 没区分） | 提交大小（私有部分） |
//! | `fonts_src_bytes` | `crate::fonts::embedded_bytes()` + `cjk_bytes()` | 字体源字节（非 atlas） |
//! | `drafts_bytes` | `crate::app::FerricApp::persist().drafts` 总字节 | 与 startup_diag 同口径 |
//! | `persist_bytes` | `Persist` 序列化字节 | 同上 |
//!
//! Unix 上 `ws/rss` 单位在 macOS 是 bytes、Linux 是 KB；`os_memory` 内部归一化
//! 到 bytes。

use std::path::PathBuf;
use std::time::Instant;

/// 单次采样的所有数值。全部字节；调用方负责选单位（MB/KB）。
#[derive(Clone, Copy, Debug, Default)]
pub struct MemSnapshot {
    /// 距 recording 开始的毫秒数。
    pub elapsed_ms: u64,
    /// 进程私有工作集（任务管理器·内存·私有工作集）。失败为 0。
    pub ws_bytes: u64,
    /// 进程私有提交字节（Windows: PagefileUsage；Unix: 0）。失败为 0。
    pub rss_bytes: u64,
    /// 字体源字节总数（embedded 设计字体 + CJK 加载字节）。
    /// **不是 atlas 实际字节**——egui 没公开 API 给 atlas 大小。
    pub fonts_src_bytes: u64,
    /// 各工具草稿的字节总和，与 `startup_diag` 同口径（从 `Persist.drafts`）。
    pub drafts_bytes: u64,
    /// `Persist` 序列化后的字节数。
    pub persist_bytes: u64,
}

/// 一次 30 秒录制窗口。`tick()` 在每帧调一次，30 秒到点后 `tick()` 返回 `true`
/// 通知调用方落盘。
pub struct MemoryRecorder {
    started: Instant,
    duration_ms: u64,
    next_tick_ms: u64,
    samples: Vec<MemSnapshot>,
    sink_path: PathBuf,
    backend_label: String,
}

impl MemoryRecorder {
    /// 启动一次录制。`data_dir` 拿不到（极端冷启动场景）时返回 `None`。
    pub fn start(data_dir: &std::path::Path, backend: &str) -> Self {
        let sink = data_dir.join("memory.log");
        Self {
            started: Instant::now(),
            duration_ms: 30_000,
            next_tick_ms: 0,
            samples: Vec::with_capacity(32),
            sink_path: sink,
            backend_label: backend.to_string(),
        }
    }

    /// 录制时长（秒），暴露给 UI 显示「正在记录… 23 / 30 秒」。
    pub fn duration_secs(&self) -> u64 {
        self.duration_ms / 1000
    }

    /// 已录制秒数（向下取整），暴露给 UI 显示。
    pub fn elapsed_secs(&self) -> u64 {
        (self.elapsed_ms_so_far() / 1000).min(self.duration_secs())
    }

    /// 当前数据目录下的目标文件路径（用于日志提示）。
    pub fn sink_path(&self) -> &std::path::Path {
        &self.sink_path
    }

    fn elapsed_ms_so_far(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// 推进一步：如果已到采样时点就抓一次快照并返回 `Some(snapshot)`；
    /// 到点（30 秒已过）则返回 `None` 并通知调用方 `finish()`。
    ///
    /// `app_persist` / `app_drafts` 由调用方从 `FerricApp` 拿；`fonts` 与 OS
    /// 字段由本模块自行获取。这条签名刻意避免直接依赖 `FerricApp`——让
    /// `mem` 模块单测不需要 eframe 上下文。
    pub fn tick(
        &mut self,
        app_persist_bytes: u64,
        app_drafts_bytes: u64,
    ) -> TickOutcome {
        let now = self.elapsed_ms_so_far();
        if now < self.next_tick_ms {
            return TickOutcome::Pending;
        }
        // 推进下一次采样时点：本帧抓完后下一帧应至少等 1000ms。
        // 容忍漂移：now < next_tick_ms + 1000 时也立刻采，避免漏掉边界采样。
        self.next_tick_ms = now + 1000;

        let (ws_bytes, rss_bytes) = os_memory();
        let fonts_src_bytes = (crate::fonts::embedded_bytes() + crate::fonts::cjk_bytes()) as u64;
        let snap = MemSnapshot {
            elapsed_ms: now,
            ws_bytes,
            rss_bytes,
            fonts_src_bytes,
            drafts_bytes: app_drafts_bytes,
            persist_bytes: app_persist_bytes,
        };
        self.samples.push(snap);

        if now >= self.duration_ms {
            TickOutcome::Finished(snap)
        } else {
            TickOutcome::Sampled(snap)
        }
    }

    /// 落盘。失败原因透传给调用方，调用方负责 toast/dialog。
    pub fn finish(&self) -> Result<(), String> {
        // 父目录由 launch::data_dir() 的契约保证存在；保险起见再 mkdir 一次。
        if let Some(parent) = self.sink_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = String::with_capacity(64 + 32 * 96);
        let started_at = chrono::Utc::now();
        out.push_str(&format!(
            "[mem-rec {}] backend={} session=PID-{}\n",
            started_at.format("%Y-%m-%dT%H:%M:%SZ"),
            self.backend_label,
            std::process::id(),
        ));
        for s in &self.samples {
            out.push_str(&format!(
                "t={} ws={} rss={} fonts_src={} drafts={} persist={}\n",
                s.elapsed_ms,
                fmt_mb(s.ws_bytes),
                fmt_mb(s.rss_bytes),
                fmt_mb(s.fonts_src_bytes),
                fmt_mb(s.drafts_bytes),
                fmt_mb(s.persist_bytes),
            ));
        }
        out.push_str("[mem-rec done]\n");
        // 追加写：保留历次"记录"会话，按时间顺序串在同一个文件里。
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.sink_path)
            .map_err(|e| format!("{}: {}", self.sink_path.display(), e))?;
        f.write_all(out.as_bytes()).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// `tick()` 的返回。`Pending` 不动；`Sampled` 抓到一次；`Finished` 到点。
#[derive(Debug)]
pub enum TickOutcome {
    Pending,
    Sampled(MemSnapshot),
    Finished(MemSnapshot),
}

/// 把字节数格式成 `12.3MB` 这种 1 位小数（与 `app::fmt_bytes` 同风格，但单位
/// 固定 MB——内存采样没必要再区分 B/KB）。
fn fmt_mb(n: u64) -> String {
    const UNIT: u64 = 1024 * 1024;
    if n == 0 {
        return "0B".to_string();
    }
    if n < UNIT {
        return format!("{}B", n);
    }
    let mb = n as f64 / UNIT as f64;
    format!("{:.1}MB", mb)
}

// ============ OS 层：进程私有工作集 / 私有提交字节 ============

#[cfg(target_os = "windows")]
fn os_memory() -> (u64, u64) {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut info: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let handle = unsafe { GetCurrentProcess() };
    // windows_sys 0.59 的 GetProcessMemoryInfo 第一个参数是 HANDLE；
    // 这层 unsafe 由本函数边界承担。
    let ok = unsafe { GetProcessMemoryInfo(handle, &mut info, size) };
    if ok == 0 {
        return (0, 0);
    }
    (info.WorkingSetSize as u64, info.PagefileUsage as u64)
}

#[cfg(target_os = "linux")]
fn os_memory() -> (u64, u64) {
    // Linux 下优先读 /proc/self/statm：粒度粗（页），但 0 syscall 开销、无 libc 依赖。
    // Linux 上 ru_maxrss 单位是 KB；fallback 用它。
    if let Some((ws, rss)) = read_proc_self_status() {
        return (ws, rss);
    }
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    if ok != 0 {
        return (0, 0);
    }
    let kb = ru.ru_maxrss as u64;
    (kb * 1024, 0)
}

#[cfg(target_os = "linux")]
fn read_proc_self_status() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let mut ws: u64 = 0;
    let mut rss: u64 = 0;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss = parse_kb(rest).unwrap_or(0) * 1024;
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            ws = parse_kb(rest).unwrap_or(0) * 1024;
        }
    }
    if rss == 0 && ws == 0 {
        None
    } else if ws == 0 {
        // 没拿到 VmHWM（高水位）就用当前 RSS 顶上 ws，避免字段全 0。
        (rss, rss).into()
    } else {
        Some((ws, rss))
    }
}

#[cfg(target_os = "linux")]
fn parse_kb(s: &str) -> Option<u64> {
    s.split_whitespace().next()?.parse().ok()
}

#[cfg(target_os = "macos")]
fn os_memory() -> (u64, u64) {
    // macOS 没有 WMI/procfs；getrusage 的 ru_maxrss 单位是 bytes。
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    if ok != 0 {
        return (0, 0);
    }
    (ru.ru_maxrss as u64, 0)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn os_memory() -> (u64, u64) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_mb_zero() {
        assert_eq!(fmt_mb(0), "0B");
    }

    #[test]
    fn fmt_mb_under_unit() {
        assert_eq!(fmt_mb(512), "512B");
    }

    #[test]
    fn fmt_mb_normal() {
        let s = fmt_mb(128 * 1024 * 1024);
        assert_eq!(s, "128.0MB");
    }

    #[test]
    fn fmt_mb_fractional() {
        let s = fmt_mb((12.5 * 1024.0 * 1024.0) as u64);
        assert!(s.starts_with("12.5MB"), "got {s}");
    }

    #[test]
    fn recorder_tick_pending_until_due() {
        let dir = std::env::temp_dir().join("ferric-mem-test-pending");
        std::fs::create_dir_all(&dir).unwrap();
        let mut rec = MemoryRecorder::start(&dir, "test");
        // 立刻 tick：now 几乎为 0，next_tick_ms 初值 0，应立刻采样。
        match rec.tick(100, 200) {
            TickOutcome::Sampled(s) | TickOutcome::Finished(s) => {
                assert_eq!(s.drafts_bytes, 200);
                assert_eq!(s.persist_bytes, 100);
            }
            TickOutcome::Pending => panic!("expected first tick to sample"),
        }
        // 紧接第二次 tick：now 还远未到 next_tick_ms，应 Pending。
        assert!(matches!(rec.tick(0, 0), TickOutcome::Pending));
    }

    #[test]
    fn recorder_finishes_at_duration() {
        let dir = std::env::temp_dir().join("ferric-mem-test-finish");
        std::fs::create_dir_all(&dir).unwrap();
        let mut rec = MemoryRecorder {
            started: Instant::now(),
            duration_ms: 0, // 立即到期
            next_tick_ms: 0,
            samples: Vec::new(),
            sink_path: dir.join("memory.log"),
            backend_label: "test".into(),
        };
        // duration=0 → 第一次 tick 就该 Finished。
        match rec.tick(0, 0) {
            TickOutcome::Finished(s) => assert_eq!(s.elapsed_ms, 0),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[test]
    fn recorder_writes_append_file() {
        let dir = std::env::temp_dir().join("ferric-mem-test-append");
        std::fs::create_dir_all(&dir).unwrap();
        // 清掉前一次的尾巴。
        let _ = std::fs::remove_file(dir.join("memory.log"));

        let mut rec = MemoryRecorder {
            started: Instant::now(),
            duration_ms: 0,
            next_tick_ms: 0,
            samples: Vec::new(),
            sink_path: dir.join("memory.log"),
            backend_label: "test".into(),
        };
        let _ = rec.tick(1, 2);
        rec.finish().unwrap();

        // 再来一次。
        let mut rec2 = MemoryRecorder {
            started: Instant::now(),
            duration_ms: 0,
            next_tick_ms: 0,
            samples: Vec::new(),
            sink_path: dir.join("memory.log"),
            backend_label: "test".into(),
        };
        let _ = rec2.tick(3, 4);
        rec2.finish().unwrap();

        let s = std::fs::read_to_string(dir.join("memory.log")).unwrap();
        // 收尾以换行收口——done 前面会出现 [mem-rec 但需要按行精确比对。
        let rec_count = s
            .lines()
            .filter(|l| l.starts_with("[mem-rec ") && !l.ends_with("done]"))
            .count();
        let done_count = s.lines().filter(|l| *l == "[mem-rec done]").count();
        assert_eq!(rec_count, 2);
        assert_eq!(done_count, 2);
    }
}