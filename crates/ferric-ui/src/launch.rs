//! 启动期配置与诊断。
//!
//! # 迁 Slint 后这个模块瘦了一大圈
//!
//! egui 时代这里有 1064 行，绝大部分在解决**一个问题**：wgpu 挑哪个后端。
//! 无 GPU 驱动的环境下 DX12 会退化成 WARP、Vulkan / OpenGL 干脆没有适配器，
//! 具体哪个能用只有到了那台机器上才知道，所以有一整套「按顺序试 + 跨启动
//! 自愈 + 软件渲染单独归类」的状态机（`plan` / `begin` / `resolve_after_success`）。
//!
//! Slint 的 `renderer-software` 让这个问题**整体消失**：它不碰任何 GPU API，
//! 任何机器上行为一致，没有「挑不到适配器」这回事。于是那套自愈连同
//! `Backend` 枚举、`WGPU_BACKEND` 环境变量、`failed` / `slow` 黑名单一起删掉。
//!
//! 保留下来的是**与渲染无关**的三件事：
//!
//! 1. 数据目录（`launch.json` / `startup.log` / `app.ron` / `memory.log` 都在这）；
//! 2. `startup.log`：发行版是 `windows_subsystem = "windows"`，stderr 没有去处，
//!    启动失败时这个文件是用户唯一能拿到的线索；
//! 3. 打开数据目录 / 重启自己 / 启动失败弹窗。
//!
//! 任何一步失败（目录建不了、文件写不出）都**只当作没有配置**继续走 ——
//! 这个模块的存在是为了让应用更容易打开，它自己绝不能成为打不开的理由。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// 应用标识。决定状态目录位置，也是 Wayland 任务栏图标靠的 app_id
///（必须与打包产物的 .desktop 文件名 = 二进制名 `ferric` 一致）。
pub const APP_ID: &str = "ferric";

/// `launch.json` 的内容。
///
/// 渲染后端相关的字段全部删掉了（见模块文档）。剩下的是「窗口还没建出来
/// 之前就要知道」的设置 —— 这类东西不能放 `app.ron`，那份要等窗口起来才读。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LaunchCfg {
    /// 上一次启动失败的原因（设置页展示，便于用户看见到底缺什么）。
    #[serde(default)]
    pub last_error: Option<String>,
    /// 正在尝试、尚未确认成功启动。画出第一帧后清空。
    ///
    /// 保留这一位是因为它与渲染后端无关：进程在建窗路上直接死掉（缺系统库、
    /// 权限问题、显示服务器连不上）时，下次启动能据此在 `startup.log` 里
    /// 留下「上次没能起来」，而不是让用户面对一个静默失败。
    #[serde(default)]
    pub pending: bool,
}

/// 状态目录。
///
/// egui 时代这是 `eframe::storage_dir(APP_ID)`。eframe 没了之后自己算，
/// **路径必须与它一致**，否则老用户的设置、草稿、插件目录全部找不到：
///
/// | 平台 | 目录 |
/// |---|---|
/// | Windows | `%APPDATA%\ferric\data\` |
/// | macOS | `~/Library/Application Support/ferric/` |
/// | Linux | `~/.local/share/ferric/` |
///
/// eframe 用的是 `directories::ProjectDirs::from("", "", app_id)` 的
/// `data_dir()`，Windows 上它会再拼一层 `data`。这里照抄那个行为。
fn dir() -> Option<PathBuf> {
    let pd = directories::ProjectDirs::from("", "", APP_ID)?;
    let base = pd.data_dir().to_path_buf();
    // eframe 在 Windows 上把状态放在 `<roaming>/<app>/data`；
    // directories 的 data_dir() 在 Windows 已经指向 roaming/<app>/data，
    // 其他平台就是 data_dir 本身。这里不再额外拼接。
    Some(base)
}

/// 数据目录的完整路径（含 `launch.json` / `startup.log` / `app.ron`）。
/// 设置 → 数据里「打开数据文件夹」按钮用这个。
pub fn data_dir() -> Option<PathBuf> {
    dir()
}

/// `launch.json` 的完整路径。
pub fn path() -> Option<PathBuf> {
    dir().map(|d| d.join("launch.json"))
}

/// 启动诊断日志。发行版没有控制台，启动失败时这是唯一线索。
pub fn log_path() -> Option<PathBuf> {
    dir().map(|d| d.join("startup.log"))
}

/// 追加一行启动日志（失败静默：日志写不了不该影响启动）。
///
/// 开头补一个换行的原因：egui 时代写日志的那条路径在某些分支下没写行尾换行，
/// 老用户的 `startup.log` 末尾因此可能是半行。直接 append 会把新记录粘在
/// 那半行后面（实测见过 `…忽略配置里的后端选择[2026-…] 启动`）。
/// 判一下末字节，缺换行就先补上 —— 一次 metadata 调用换来日志始终可读。
pub fn log(line: &str) {
    let Some(p) = log_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    let needs_lead_nl = std::fs::read(&p)
        .ok()
        .and_then(|b| b.last().copied())
        .is_some_and(|last| last != b'\n');
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let lead = if needs_lead_nl { "\n" } else { "" };
        let _ = writeln!(f, "{lead}[{ts}] {line}");
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

// 按路径操作的版本是为了**可测**：真实路径落在用户配置目录里，单测不该往那写。

/// 读一份配置。
///
/// 读不到 / 解析不了一律用默认值，且**不写日志** —— egui 时代那份
/// `launch.json` 是 ron 格式且字段完全不同（backend / last_good / failed / slow），
/// 老用户升级后第一次启动必然解析失败。那不是错误，是预期的一次性迁移；
/// 往 startup.log 里刷「解析失败」只会让真正的问题更难被看见。
fn load_from(p: &std::path::Path) -> LaunchCfg {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 先写临时文件再改名，避免写到一半的文件被下次启动读到。
fn save_to(p: &std::path::Path, cfg: &LaunchCfg) {
    let Ok(json) = serde_json::to_string_pretty(cfg) else {
        return;
    };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

/// 开始一次启动尝试：把「上次没能跑起来」记进日志，落下本次的 pending 标记。
pub fn begin() -> LaunchCfg {
    let mut cfg = load();
    if cfg.pending {
        // 上一次写下了 pending 却没清掉 —— 那次是崩在启动路上了。
        log("上次启动未能出帧（可能崩在建窗阶段）");
    }
    cfg.pending = true;
    save(&cfg);
    cfg
}

/// 进程内标记：UI 是否已经真的画出来过。
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 应用已经稳定出帧了。清掉 pending 标记，本次启动算成功。
///
/// egui 时代这个函数还要判断「拿到的适配器是不是软件光栅化」并据此决定
/// 下次换不换后端。Slint 走软件渲染，没有这个分支了。
pub fn mark_running() {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return; // 只有第一次生效
    }
    let mut cfg = load();
    cfg.pending = false;
    cfg.last_error = None;
    save(&cfg);
    log("已出帧，启动成功");
}

/// 本次启动是否已经成功出帧。
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// 记下一次启动失败的原因，供下次启动时在设置页展示。
pub fn mark_failed(detail: &str) {
    let mut cfg = load();
    cfg.pending = false;
    cfg.last_error = Some(detail.to_owned());
    save(&cfg);
    log(&format!("启动失败：{detail}"));
}

/// 重启本应用：拉起一份新的自己，然后请调用方关掉当前窗口。
///
/// 失败就把原因交出去，由调用方提示「请手动重启」——
/// 悄悄失败会变成「点了没反应」，那比没有这个按钮更糟。
pub fn relaunch() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("找不到程序自身路径：{e}"))?;
    std::process::Command::new(exe)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("拉起新实例失败：{e}"))
}

/// 用系统文件管理器打开数据目录。
///
/// 路径不存在也当作失败：首次启动才建目录，极端场景下也走这里，给明确错误。
pub fn open_data_dir() -> Result<(), String> {
    let p = data_dir().ok_or("取不到数据目录")?;
    if !p.exists() {
        // 首次启动、还没写过任何状态时目录可能不存在 —— 建出来再打开，
        // 比报「目录不存在」有用（用户点这个按钮就是想去那儿看看）。
        std::fs::create_dir_all(&p).map_err(|e| format!("建数据目录失败：{e}"))?;
    }
    open_in_file_manager(&p)
}

/// 平台分支的文件管理器打开。
///
/// 子进程 detach 出去后立即返回；**不**等待子进程退出（用户关掉
/// 文件管理器之前我们这边都不能卡）。
#[cfg(target_os = "windows")]
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开资源管理器失败：{e}"))
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
        .map_err(|e| format!("xdg-open 失败：{e}"))
}

/// 启动彻底失败时弹一个系统对话框。
///
/// 发行版没有控制台，不弹窗的话用户看到的就是「双击了没反应」——
/// 那是最糟的失败方式：既不知道出了什么事，也不知道下一步该干嘛。
pub fn fatal_dialog(detail: &str) {
    let log_hint = log_path()
        .map(|p| format!("\n\n详细信息已写入：\n{}", p.display()))
        .unwrap_or_default();
    let msg = format!("Ferric 启动失败。\n\n{detail}{log_hint}");
    rfd::MessageDialog::new()
        .set_title("Ferric 启动失败")
        .set_description(&msg)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferric-launch-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn config_roundtrips_through_disk() {
        let p = tmp().join("launch.json");
        let cfg = LaunchCfg {
            last_error: Some("缺少 libxkbcommon".into()),
            pending: true,
        };
        save_to(&p, &cfg);
        let got = load_from(&p);
        assert_eq!(got.last_error.as_deref(), Some("缺少 libxkbcommon"));
        assert!(got.pending);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_config_reads_as_default_not_panic() {
        // 这个文件是当前用户可写的。坏内容必须降级为默认值，
        // 否则「配置坏了」会变成「应用打不开」——那正是本模块要避免的。
        let p = tmp().join("corrupt.json");
        std::fs::write(&p, "}{ not json").unwrap();
        let got = load_from(&p);
        assert!(!got.pending);
        assert_eq!(got.last_error, None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_config_reads_as_default() {
        let got = load_from(std::path::Path::new("/nope/does/not/exist/launch.json"));
        assert!(!got.pending);
    }

    #[test]
    fn save_is_atomic_leaving_no_tmp_behind() {
        // 半个文件被下次启动读到就是一次「配置莫名回默认」。
        let p = tmp().join("atomic.json");
        save_to(&p, &LaunchCfg::default());
        assert!(p.exists());
        assert!(
            !p.with_extension("json.tmp").exists(),
            "临时文件必须已被 rename 掉"
        );
        let _ = std::fs::remove_file(&p);
    }
}