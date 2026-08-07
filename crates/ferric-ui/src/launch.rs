//! 启动期配置与自愈。
//!
//! 这里管的是「窗口还没建出来之前」的事，因此**不能**放进 eframe 的持久化状态
//! （那份要等 eframe 起来才读得到）。单独一个 `launch.json`，位置就在 eframe
//! 自己的状态文件旁边。
//!
//! 解决两个具体问题：
//!
//! 1. **渲染后端选不对就打不开 / 花屏**。无 GPU 驱动的环境（虚拟机、精简版
//!    Windows、远程桌面）下 DX12 会退化成 WARP 软件光栅化，Vulkan / OpenGL 干脆
//!    没有适配器 —— 具体哪个能用只有到了那台机器上才知道。所以：按顺序试，
//!    **谁先跑起来就记住谁**，下次直接用；用户也可以在设置里手动锁定一个。
//! 2. **上一次启动没能跑起来**。写下「正在尝试 X」，跑起来之后才清掉。下次启动
//!    如果发现这个标记还在，说明 X 那次是崩在启动路上了 —— 自动把 X 降到最后再试
//!    别的。这样即使某个后端会让进程直接死掉，用户再点一次也能进得去，
//!    而不是永远卡在同一个坑里。
//!
//! 任何一步失败（目录建不了、文件读不出、JSON 坏了）都**只当作没有配置**继续走 ——
//! 这个模块的存在是为了让应用更容易打开，它自己绝不能成为打不开的理由。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// 与 `ViewportBuilder::with_app_id` 一致；同时决定 eframe 的状态目录位置。
pub const APP_ID: &str = "ferric";

/// 渲染后端选择。`Auto` = 交给 wgpu 自己挑（默认）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Backend {
    #[default]
    Auto,
    Dx12,
    Vulkan,
    Gl,
}

impl Backend {
    /// 全部可选项（设置页按这个顺序展示）。
    pub const ALL: [Self; 4] = [Self::Auto, Self::Dx12, Self::Vulkan, Self::Gl];

    /// 传给 wgpu 的 `WGPU_BACKEND` 值；`Auto` 没有值（= 不设这个环境变量）。
    ///
    /// 名字必须是 wgpu 认的那几个（见 `wgpu::Backends::from_comma_list`），
    /// 写错了 wgpu 只会 warn 一句然后当成空集合 —— 那等于一个后端都不许用。
    pub fn env_value(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Dx12 => Some("dx12"),
            Self::Vulkan => Some("vulkan"),
            Self::Gl => Some("gl"),
        }
    }

    fn from_env_value(v: &str) -> Self {
        match v {
            "dx12" => Self::Dx12,
            "vulkan" => Self::Vulkan,
            "gl" => Self::Gl,
            _ => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::Dx12 => "DX12",
            Self::Vulkan => "Vulkan",
            Self::Gl => "OpenGL",
        }
    }
}

/// `launch.json` 的内容。字段全部 `default`，缺哪个都不影响读出来。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LaunchCfg {
    /// 用户在设置里锁定的后端。
    #[serde(default)]
    pub backend: Backend,
    /// 上一次**成功跑起来**用的后端（自动模式下优先复用）。
    #[serde(default)]
    pub last_good: Option<Backend>,
    /// 正在尝试、尚未确认成功的后端。启动画到第一帧后清空。
    #[serde(default)]
    pub pending: Option<Backend>,
    /// 试过但没能跑起来的后端（排到最后再考虑）。一旦有一次成功启动就清空 ——
    /// 驱动装上了、虚拟机开了 3D 加速，环境是会变好的，不该永久拉黑。
    #[serde(default)]
    pub failed: Vec<Backend>,
    /// 上一次启动失败的原因（设置页展示，便于用户看见到底缺什么）。
    #[serde(default)]
    pub last_error: Option<String>,
}

fn dir() -> Option<PathBuf> {
    eframe::storage_dir(APP_ID)
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

fn set_backend_at(p: &std::path::Path, backend: Backend) -> LaunchCfg {
    let mut cfg = load_from(p);
    cfg.backend = backend;
    cfg.failed.clear();
    save_to(p, &cfg);
    cfg
}

/// 后端的优先顺序，第一个就是**本次**要用的。
///
/// ⚠️ 为什么不是「一次启动里从头试到尾」：winit 全进程只允许创建一次事件循环
/// （`EventLoopError::RecreationAttempt`），`run_native` 失败之后在同一个进程里
/// 再来一次必然失败。所以自愈是**跨启动**的：本次用第一个，失败/崩溃就记进
/// `failed`，下次自然轮到下一个。别把这里改回循环重试。
///
/// 规则（从前往后）：
/// 1. 用户锁定了某个后端 → 它排第一（尊重设置），但仍保留其余作为兜底 ——
///    「我选的那个这台机器上根本没有」不该变成打不开；
/// 2. 否则上次成功的那个排第一（行为稳定，也省掉一次全量枚举）；
/// 3. 然后是默认顺序：先 `Auto`（wgpu 自己挑，绝大多数机器到这就结束了），
///    再逐个点名；
/// 4. 试过没跑起来的（`failed`）一律降到最后。
pub fn plan(cfg: &LaunchCfg) -> Vec<Backend> {
    fn push(order: &mut Vec<Backend>, b: Backend) {
        if !order.contains(&b) {
            order.push(b);
        }
    }
    let mut order: Vec<Backend> = Vec::with_capacity(Backend::ALL.len());
    if cfg.backend != Backend::Auto {
        push(&mut order, cfg.backend);
    }
    if let Some(b) = cfg.last_good {
        push(&mut order, b);
    }
    for b in Backend::ALL {
        push(&mut order, b);
    }
    // 崩在启动路上的那些，挪到最后再说（保持它们之间的先后）
    for bad in cfg.failed.iter().copied() {
        if order.contains(&bad) {
            order.retain(|b| *b != bad);
            order.push(bad);
        }
    }
    order
}

/// 开始一次启动尝试：把上一次没能跑起来的记进 `failed`，挑出本次要用的后端，
/// 设好环境变量并落盘「正在尝试」标记。返回本次使用的后端。
pub fn begin(cfg: &mut LaunchCfg) -> Backend {
    if let Some(bad) = cfg.pending.take() {
        log(&format!(
            "上次以 {} 启动没能出帧（崩溃或卡死），本次把它排到最后",
            bad.label()
        ));
        if !cfg.failed.contains(&bad) {
            cfg.failed.push(bad);
        }
    }
    let backend = plan(cfg).first().copied().unwrap_or_default();
    apply(backend);
    cfg.pending = Some(backend);
    save(cfg);
    backend
}

/// 让本进程接下来创建的 wgpu 实例使用指定后端。
///
/// 走 `WGPU_BACKEND` 环境变量而不是 `WgpuConfiguration`：egui-wgpu 的默认设置本来
/// 就是 `Backends::from_env().unwrap_or(PRIMARY | GL)`，用环境变量等于只覆盖那一个
/// 字段，其余（display handle、设备描述符、适配器选择）全部保持 eframe 的默认路径。
///
/// 必须在构造 `NativeOptions` **之前**调用 —— 那个默认值就是在构造时读的环境变量。
pub fn apply(backend: Backend) {
    match backend.env_value() {
        Some(v) => std::env::set_var("WGPU_BACKEND", v),
        // 自动模式要把可能残留的值清掉（同一进程里我们会连试几个）
        None => std::env::remove_var("WGPU_BACKEND"),
    }
}

/// 改「用户锁定的渲染后端」这一项，返回落盘后的完整配置。
///
/// 必须**先重读磁盘**再改：应用内存里的那份是启动那一刻读的，之后
/// [`mark_running`] 还往盘上写过（清 `pending` / `failed`、记 `last_good`）。
/// 直接把内存里的旧快照存回去会把那些结果抹掉 —— 表现为「明明这次跑得好好的，
/// 下次启动却说上次崩了」。
///
/// 同时清空 `failed`：用户是在主动指定，不该被上一次的失败记录压着不用。
pub fn set_backend(backend: Backend) -> LaunchCfg {
    match path() {
        Some(p) => set_backend_at(&p, backend),
        None => LaunchCfg {
            backend,
            ..Default::default()
        },
    }
}

/// 进程内标记：UI 是否已经真的画出来过。
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 应用已经稳定出帧了 —— 把本次用的后端记成 `last_good`，清掉 `pending` 与
/// 黑名单（环境是会变好的：装了驱动、开了 3D 加速，不该永久拉黑一个后端）。
///
/// 由 `FerricApp` 在头几帧之后调用一次（只有第一次生效）。
pub fn mark_running() {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut cfg = load();
    let used = cfg.pending.unwrap_or_else(|| {
        std::env::var("WGPU_BACKEND")
            .ok()
            .map_or(Backend::Auto, |v| Backend::from_env_value(&v))
    });
    cfg.last_good = Some(used);
    cfg.pending = None;
    cfg.failed.clear();
    cfg.last_error = None;
    save(&cfg);
}

/// 本次启动是否已经成功出帧。
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
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
            "已尝试全部渲染后端（自动 / DX12 / Vulkan / OpenGL）仍无法创建窗口。\n\n\
             通常是显卡驱动缺失或过旧。请更新显卡驱动后重试；\
             虚拟机 / 远程桌面下可尝试开启 3D 加速。\n\n\
             最后一次的错误：\n{detail}{log_hint}"
        ))
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户锁定的后端必须排第一 —— 设置里选了什么，就先用什么。
    #[test]
    fn user_choice_goes_first() {
        let cfg = LaunchCfg {
            backend: Backend::Gl,
            ..Default::default()
        };
        assert_eq!(plan(&cfg).first(), Some(&Backend::Gl));
    }

    /// 但锁定不等于「只用它」：它要是起不来，后面仍得有兜底，
    /// 否则一个选错的设置就能让应用永远打不开。
    #[test]
    fn locking_a_backend_still_keeps_fallbacks() {
        let cfg = LaunchCfg {
            backend: Backend::Vulkan,
            ..Default::default()
        };
        let p = plan(&cfg);
        assert!(p.len() >= Backend::ALL.len(), "锁定后端之后没有兜底：{p:?}");
        assert!(p.contains(&Backend::Auto));
    }

    /// 上次成功的后端优先复用（自动模式下省掉重新枚举）。
    #[test]
    fn last_good_is_preferred_in_auto_mode() {
        let cfg = LaunchCfg {
            last_good: Some(Backend::Dx12),
            ..Default::default()
        };
        assert_eq!(plan(&cfg).first(), Some(&Backend::Dx12));
    }

    /// 没能跑起来的那个降到最后 —— 这是「崩了还能再进去」的关键。
    #[test]
    fn a_backend_that_failed_is_demoted() {
        let cfg = LaunchCfg {
            last_good: Some(Backend::Dx12),
            failed: vec![Backend::Dx12],
            ..Default::default()
        };
        let p = plan(&cfg);
        assert_eq!(
            p.last(),
            Some(&Backend::Dx12),
            "崩过的后端没被降到最后：{p:?}"
        );
        assert_ne!(p.first(), Some(&Backend::Dx12));
    }

    /// 连续崩溃要能**逐个换过去**收敛，而不是在同两个之间来回打转。
    ///
    /// 模拟：每次启动都崩 —— 计划的头一个不断变化，四次之内必须把四种都试过。
    #[test]
    fn repeated_crashes_walk_through_every_backend() {
        let mut cfg = LaunchCfg::default();
        let mut tried = Vec::new();
        for _ in 0..Backend::ALL.len() {
            // begin() 的纯逻辑部分：把上次的 pending 记进 failed，再取计划首项
            if let Some(bad) = cfg.pending.take() {
                if !cfg.failed.contains(&bad) {
                    cfg.failed.push(bad);
                }
            }
            let b = plan(&cfg).first().copied().unwrap();
            assert!(!tried.contains(&b), "又挑到了试过的 {b:?}：{tried:?}");
            tried.push(b);
            cfg.pending = Some(b); // 又崩了
        }
        assert_eq!(tried.len(), Backend::ALL.len(), "没能把所有后端都试到");
    }

    /// 计划里不能有重复项，否则同一个后端会被白试两遍。
    #[test]
    fn plan_has_no_duplicates() {
        let cfg = LaunchCfg {
            backend: Backend::Gl,
            last_good: Some(Backend::Gl),
            failed: vec![Backend::Auto, Backend::Gl],
            ..Default::default()
        };
        let p = plan(&cfg);
        let seen: std::collections::HashSet<&Backend> = p.iter().collect();
        assert_eq!(seen.len(), p.len(), "启动计划里有重复：{p:?}");
    }

    /// 在设置里换后端**只能**动 `backend` 与 `failed` 两项。
    ///
    /// 盘上那份在应用启动之后还被 `mark_running` 改过（记 `last_good`、清 `pending`），
    /// 若把内存里的旧快照整份存回去，就会把那些结果抹掉 —— 表现为「这次明明跑得
    /// 好好的，下次启动却说上次崩了」。
    #[test]
    fn changing_the_backend_preserves_what_the_app_wrote_meanwhile() {
        let dir = std::env::temp_dir().join(format!("ferric-launch-test-{}", std::process::id()));
        let p = dir.join("launch.json");
        let _ = std::fs::remove_dir_all(&dir);

        // 应用跑起来之后盘上的样子
        save_to(
            &p,
            &LaunchCfg {
                backend: Backend::Auto,
                last_good: Some(Backend::Dx12),
                pending: None,
                failed: vec![Backend::Vulkan],
                last_error: None,
            },
        );

        let after = set_backend_at(&p, Backend::Gl);
        assert_eq!(after.backend, Backend::Gl, "没记下用户选的后端");
        assert!(after.failed.is_empty(), "主动指定后端时没清掉黑名单");
        assert_eq!(
            after.last_good,
            Some(Backend::Dx12),
            "把应用写的 last_good 抹掉了"
        );
        // 回读一遍，确认真的落盘了
        assert_eq!(load_from(&p).backend, Backend::Gl);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏掉 / 半截的配置文件不能让启动出问题：一律当默认值。
    #[test]
    fn a_corrupt_config_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("ferric-launch-bad-{}", std::process::id()));
        let p = dir.join("launch.json");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(&p, "{ this is not json").unwrap();
        let cfg = load_from(&p);
        assert_eq!(cfg.backend, Backend::Auto);
        assert!(cfg.failed.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 后端名必须是 wgpu 认得的那几个：写错了 wgpu 会解析成空集合，
    /// 结果是「一个后端都不许用」——比不设还糟。
    #[test]
    fn env_values_are_the_names_wgpu_understands() {
        for b in Backend::ALL {
            if let Some(v) = b.env_value() {
                assert!(
                    matches!(v, "dx12" | "vulkan" | "gl" | "metal"),
                    "{b:?} 的 WGPU_BACKEND 值 `{v}` 不是 wgpu 认的名字"
                );
                assert_eq!(Backend::from_env_value(v), b, "{b:?} 的名字回读不一致");
            }
        }
    }
}
