//! 启动期配置：选渲染方式，打不开时自动退到 CPU 渲染。
//!
//! 只有两种渲染方式：
//!
//! - **OpenGL（glow）**：默认。eframe 里最轻的显卡渲染器，有显卡的机器省 CPU。
//! - **CPU 渲染**（`ferric-soft-render`）：不碰任何显卡接口，任何机器都能开。
//!
//! 规则只有一条：**OpenGL 打不开就改用 CPU 渲染，并记住**。
//!
//! - 建窗直接报错 → `main` 记下 `soft = true` 后重新拉起自己（winit 全进程只允许
//!   一次事件循环，同一进程里没法换个渲染器重来）。
//! - 进程直接崩掉（驱动问题）→ 启动前写下的 `glow_pending` 标记没被清掉，
//!   下次启动看到它就改用 CPU 渲染。
//!
//! 用户可以在设置里手动切回来。配置单独放在 `launch.json`（eframe 状态目录里），
//! 因为它要在建窗之前读。任何一步读写失败都只当作没有配置 —— 这个模块绝不能
//! 成为打不开的理由。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// 与 `ViewportBuilder::with_app_id` 一致；同时决定 eframe 的状态目录位置。
pub const APP_ID: &str = "ferric";

/// 渲染方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    /// OpenGL（eframe 的 glow 渲染器）。
    Glow,
    /// 纯 CPU 渲染，不建任何显卡上下文。
    Soft,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Glow => "OpenGL",
            Self::Soft => "CPU 渲染",
        }
    }
}

/// `launch.json` 的内容。字段全部 `default`：缺哪个、多哪个都不影响读出来
/// （老版本留下的 `backend` / `failed` 等字段会被直接忽略）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LaunchCfg {
    /// 用 CPU 渲染（用户在设置里选的，或 OpenGL 打不开后自动改的）。
    #[serde(default)]
    pub soft: bool,
    /// 正在用 OpenGL 启动、还没画出第一帧。下次启动时它还在 = 上次崩在启动路上。
    #[serde(default)]
    pub glow_pending: bool,
    /// 最近一次自动改用 CPU 渲染的原因（设置页展示）。
    #[serde(default)]
    pub last_error: Option<String>,
}

impl LaunchCfg {
    pub fn backend(&self) -> Backend {
        if self.soft {
            Backend::Soft
        } else {
            Backend::Glow
        }
    }
}

fn dir() -> Option<PathBuf> {
    eframe::storage_dir(APP_ID)
}

/// eframe 持久化目录的完整路径（同时含 `launch.json` 与 `startup.log`）。
/// 设置 → 关于里「打开数据文件夹」按钮用这个。
pub fn data_dir() -> Option<PathBuf> {
    dir()
}

/// `launch.json` 的完整路径。
pub fn path() -> Option<PathBuf> {
    dir().map(|d| d.join("launch.json"))
}

/// 启动诊断日志。发行版是 `windows_subsystem = "windows"`，stderr 没有任何去处，
/// 启动失败时这个文件是用户唯一能拿到的线索。
pub fn log_path() -> Option<PathBuf> {
    dir().map(|d| d.join("startup.log"))
}

/// 追加一行启动日志（失败静默：日志写不了不该影响启动）。
pub fn log(line: &str) {
    use std::io::Write as _;
    let Some(p) = log_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 只留最近一段，别让它无限长
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > 64 * 1024) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// 读配置。读不到 / 解析不了一律当作默认值 —— 坏文件不该让人打不开应用。
pub fn load() -> LaunchCfg {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

/// 写配置。
pub fn save(cfg: &LaunchCfg) {
    if let Some(p) = path() {
        save_to(&p, cfg);
    }
}

// 下面三个按路径操作的版本是为了**可测**：真实路径落在用户的配置目录里，
// 单测不该往那儿写东西。公开接口只是它们套上 `path()` 的外壳。

fn load_from(p: &std::path::Path) -> LaunchCfg {
    match std::fs::read_to_string(p) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log(&format!("launch.json 解析失败（已按默认值继续）：{e}"));
            LaunchCfg::default()
        }),
        Err(_) => LaunchCfg::default(),
    }
}

/// 先写临时文件再改名，避免写到一半的文件被下次启动读到。
fn save_to(p: &std::path::Path, cfg: &LaunchCfg) {
    if let Some(parent) = p.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(text) = serde_json::to_string_pretty(cfg) else {
        return;
    };
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// 开始一次启动：决定本次用哪种渲染方式，并落盘「正在尝试 OpenGL」标记。
///
/// 环境变量 `FERRIC_RENDERER=soft|glow` 可临时覆盖配置（排障用，不落盘）。
pub fn begin(cfg: &mut LaunchCfg) -> Backend {
    let env = std::env::var("FERRIC_RENDERER").ok();
    let backend = choose(cfg, env.as_deref());
    save(cfg);
    backend
}

/// [`begin`] 的纯逻辑部分（可测：不碰磁盘、不碰环境变量）。
fn choose(cfg: &mut LaunchCfg, env: Option<&str>) -> Backend {
    if cfg.glow_pending {
        log("上次以 OpenGL 启动没能出帧（崩溃或卡死），本次改用 CPU 渲染");
        cfg.glow_pending = false;
        cfg.soft = true;
        cfg.last_error = Some("OpenGL 上次启动时崩溃，已自动改用 CPU 渲染".to_owned());
    }
    let backend = match env {
        Some("soft") => Backend::Soft,
        Some("glow") => Backend::Glow,
        _ => cfg.backend(),
    };
    cfg.glow_pending = backend == Backend::Glow;
    backend
}

/// OpenGL 建窗失败：改用 CPU 渲染并落盘。由 `main` 在重新拉起自己之前调用。
pub fn fall_back_to_soft(cfg: &mut LaunchCfg, detail: &str) {
    cfg.soft = true;
    cfg.glow_pending = false;
    cfg.last_error = Some(format!("OpenGL 无法启动，已自动改用 CPU 渲染：{detail}"));
    save(cfg);
}

/// 设置里切换渲染方式，返回落盘后的完整配置（重启后生效）。
///
/// 先重读磁盘再改：内存里那份是启动时读的，之后 [`mark_running`] 还写过盘。
pub fn set_soft(soft: bool) -> LaunchCfg {
    let Some(p) = path() else {
        return LaunchCfg {
            soft,
            ..Default::default()
        };
    };
    let mut cfg = load_from(&p);
    cfg.soft = soft;
    cfg.last_error = None;
    save_to(&p, &cfg);
    cfg
}

/// 进程内标记：UI 是否已经真的画出来过。
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 应用已经稳定出帧：清掉「正在尝试 OpenGL」标记。只有第一次调用生效。
pub fn mark_running() {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut cfg = load();
    if cfg.glow_pending {
        cfg.glow_pending = false;
        save(&cfg);
    }
}

/// 本次启动是否已经成功出帧。
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// 重启本应用：拉起一份新的自己，然后请调用方关掉当前窗口。
///
/// 渲染方式只能在建窗**之前**决定，所以换渲染方式必然要重启。没有这个按钮的话，「重启后生效」就是把活儿丢回给用户：
/// 他得自己找到窗口关掉、再去开始菜单点开 —— 而他正卡着，最需要的恰恰是马上看到效果。
///
/// 失败就把原因交出去，由调用方提示「请手动重启」——
/// 悄悄失败会变成「点了没反应」，那比没有这个按钮更糟。
pub fn relaunch() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("找不到程序自身路径：{e}"))?;
    // 不继承命令行参数：本应用没有会影响启动的参数，而原样传递反而可能
    // 把上一次的一次性调试开关（如 FERRIC_SCREENSHOT 配套的用法）带进新进程。
    std::process::Command::new(&exe)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("拉起新进程失败：{e}"))
}

/// 用系统文件管理器打开数据目录（`launch.json` 与 `startup.log` 的位置）。
///
/// 设置 → 关于里那个「打开数据文件夹」按钮调这个。失败时返回原因，
/// 由调用方 toast / dialog 展示 —— 静默失败等同于「点了没反应」，
/// 那比没有这个按钮更糟。
///
/// 路径不存在也当作失败：启动后第一次写 launch.json 才会建目录，
/// 极端场景（首次启动 + 目录建失败）下也走这里，那时候给一个明确错误。
pub fn open_data_dir() -> Result<(), String> {
    let path = data_dir().ok_or_else(|| "找不到数据目录".to_owned())?;
    if !path.exists() {
        return Err(format!("目录不存在：{}", path.display()));
    }
    open_in_file_manager(&path)
}

/// 平台分支的文件管理器打开。
///
/// 子进程 detach 出去后立即返回；**不**等待子进程退出（用户关掉
/// 文件管理器之前我们这边都不能卡）。失败透传 errno 由调用方包装成中文。
#[cfg(target_os = "windows")]
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    // `explorer.exe <dir>` 会新建一个窗口并定位到该目录。如果该目录已开着
    // 一个文件管理器窗口，行为是开新窗口（不会复用旧窗口）—— 一致且好懂。
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动资源管理器失败：{e}"))
}

#[cfg(target_os = "macos")]
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开 Finder 失败：{e}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开文件管理器失败：{e}"))
}

/// 启动彻底失败时弹一个系统对话框。
///
/// 发行版没有控制台，不弹窗的话用户看到的就是「双击了没反应」——
/// 那是最糟的失败方式：既不知道出了什么事，也不知道下一步该干嘛。
pub fn fatal_dialog(detail: &str) {
    let log_hint = log_path()
        .map(|p| format!("\n\n诊断日志：{}", p.display()))
        .unwrap_or_default();
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Ferric 无法启动")
        .set_description(format!(
            "OpenGL 与 CPU 渲染都无法创建窗口。\n\n\
             最后一次的错误：\n{detail}{log_hint}"
        ))
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferric-launch-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("launch.json")
    }

    /// 全新安装默认用 OpenGL。
    #[test]
    fn a_fresh_install_starts_on_glow() {
        assert_eq!(LaunchCfg::default().backend(), Backend::Glow);
    }

    /// 老版本的 launch.json（字段完全不同）要能读，且按默认值走 OpenGL。
    #[test]
    fn old_launch_json_is_ignored_gracefully() {
        let p = tmp("old");
        std::fs::write(
            &p,
            r#"{"backend":"Soft","last_good":"Dx12","pending":"Dx12","failed":[],"slow":[]}"#,
        )
        .unwrap();
        let cfg = load_from(&p);
        assert!(!cfg.soft);
        assert!(!cfg.glow_pending);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    /// 上次 OpenGL 没画出第一帧（标记还在）→ 本次改用 CPU 渲染，且不再标记 pending。
    #[test]
    fn a_crashed_glow_start_falls_back_to_soft() {
        let mut cfg = LaunchCfg {
            glow_pending: true,
            ..Default::default()
        };
        assert_eq!(choose(&mut cfg, None), Backend::Soft);
        assert!(cfg.soft);
        assert!(!cfg.glow_pending);
        assert!(cfg.last_error.is_some());
    }

    /// 用 OpenGL 启动时要落下 pending 标记，否则崩了下次也不会自动切换。
    #[test]
    fn a_glow_start_is_marked_pending() {
        let mut cfg = LaunchCfg::default();
        assert_eq!(choose(&mut cfg, None), Backend::Glow);
        assert!(cfg.glow_pending);
    }

    /// 环境变量可以临时覆盖配置。
    #[test]
    fn env_overrides_cfg() {
        let mut cfg = LaunchCfg::default();
        assert_eq!(choose(&mut cfg, Some("soft")), Backend::Soft);
        assert!(!cfg.glow_pending);
        assert!(!cfg.soft, "环境变量只管本次，不改配置");
    }

    /// 配置能落盘并读回。
    #[test]
    fn cfg_round_trips() {
        let p = tmp("rt");
        let cfg = LaunchCfg {
            soft: true,
            glow_pending: false,
            last_error: Some("x".into()),
        };
        save_to(&p, &cfg);
        let back = load_from(&p);
        assert!(back.soft);
        assert_eq!(back.last_error.as_deref(), Some("x"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
