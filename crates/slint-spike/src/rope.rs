//! Rope 风险验证：Slint 原生 `TextEdit` 能不能扛住 ferric 的 JSON 工具。
//!
//! egui 时代为此专门写了 `egui-rope-editor`（rope 存储 + 视口虚拟化 + 增量布局），
//! 因为 egui 的 `TextEdit` 每帧把整个字符串重新布局，5MB 直接卡死。
//! 这个 probe 回答的是：Slint 换了哪一种失败方式、边界在哪。
//!
//! # 用法
//!
//! ```text
//! rope-spike <行数>
//! ```
//!
//! 单次运行：造 N 行 JSON → 灌进 TextEdit → 渲染两帧 → 量内存与开头插入耗时 →
//! 退出码 0。**如果 Slint 在布局阶段 panic，进程非 0 退出** —— 调用方据此二分
//! 出可用上界。行数而不是字节数作参数：panic 来自布局高度（行数 × 行高），
//! 与字节数只是间接相关。
//!
//! 结果打到 stdout，同时追加到 `%TEMP%/ferric-spike/rope.log`。

slint::include_modules!();

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

#[cfg(target_os = "windows")]
fn ws_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut info = std::mem::MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), info.as_mut_ptr(), size) };
    if ok == 0 {
        return 0;
    }
    unsafe { info.assume_init().WorkingSetSize as u64 }
}

#[cfg(not(target_os = "windows"))]
fn ws_bytes() -> u64 {
    0
}

fn mb(n: u64) -> f64 {
    n as f64 / (1024.0 * 1024.0)
}

fn report(line: &str) {
    println!("{line}");
    let dir = std::env::temp_dir().join("ferric-spike");
    let _ = std::fs::create_dir_all(&dir);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("rope.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// 造 N 行形状接近真实的 JSON（对象数组，每行约 190 字节）。
///
/// 不用 `"x".repeat()` —— 单行超长文本和多行文本在布局器里是两个完全不同的
/// 问题，而 ferric 用户手里的是多行 JSON。
fn make_json(lines: usize) -> String {
    let mut s = String::with_capacity(lines * 200 + 16);
    s.push_str("[\n");
    for i in 0..lines.saturating_sub(2) {
        s.push_str(&format!(
            "  {{\"id\": {i}, \"name\": \"item-{i:06}\", \"uuid\": \"3f2504e0-4f89-11d3-9a0c-{i:012}\", \"tags\": [\"alpha\", \"beta\", \"gamma\"], \"score\": {}.{}, \"active\": {}}}{}\n",
            i % 100,
            i % 1000,
            i % 2 == 0,
            if i + 3 < lines { "," } else { "" }
        ));
    }
    s.push_str("]\n");
    s
}

fn main() -> Result<(), slint::PlatformError> {
    use slint::ComponentHandle;

    let lines: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    slint::BackendSelector::new()
        .renderer_name("software".into())
        .select()?;

    let win = RopeSpike::new()?;
    win.show()?;

    let baseline = ws_bytes();
    let text = make_json(lines);
    let bytes = text.len();
    let real_lines = text.lines().count();

    report(&format!(
        "--- probe lines={real_lines} bytes={:.2}MB baseline_ws={:.1}MB",
        mb(bytes as u64),
        mb(baseline)
    ));

    let t_set = Instant::now();
    win.set_payload(text.into());
    let set_ms = t_set.elapsed().as_secs_f64() * 1000.0;
    win.set_stats(format!("{real_lines} 行 / {:.2} MB", mb(bytes as u64)).into());

    // 排两步：第一次 tick 时首帧已经画完（布局 panic 会在那之前发生），
    // 第二次 tick 量开头插入。中间必须真的过一帧 —— set_payload 只改 property，
    // 布局与光栅化在下一帧才做，立刻读表读到的是假数字。
    let step = Rc::new(RefCell::new(0u32));
    let timer = Rc::new(slint::Timer::default());
    let w = win.as_weak();
    let t2 = timer.clone();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(700),
        move || {
            let Some(win) = w.upgrade() else { return };
            let n = {
                let mut guard = step.borrow_mut();
                *guard += 1;
                *guard
            };

            match n {
                1 => {
                    report(&format!(
                        "    首帧后 WS={:.1}MB 放大={:.1}× set_payload={set_ms:.0}ms",
                        mb(ws_bytes()),
                        (ws_bytes().saturating_sub(baseline)) as f64 / bytes.max(1) as f64
                    ));
                }
                2 => {
                    // 开头插入 = 最坏情况的增量编辑。这一条决定「编辑手感」。
                    let old = win.get_payload();
                    let t = Instant::now();
                    let mut s = String::with_capacity(old.len() + 1);
                    s.push('X');
                    s.push_str(old.as_str());
                    win.set_payload(s.into());
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    report(&format!("    开头插入 set={ms:.1}ms"));
                }
                _ => {
                    report(&format!("    稳态 WS={:.1}MB  [OK lines={real_lines}]", mb(ws_bytes())));
                    t2.stop();
                    let _ = slint::quit_event_loop();
                }
            }
        },
    );

    slint::run_event_loop()
}