//! 启动期配置与自愈。
//!
//! 这里管的是「窗口还没建出来之前」的事，因此**不能**放进 eframe 的持久化状态
//! （那份要等 eframe 起来才读得到）。单独一个 `launch.json`，位置就在 eframe
//! 自己的状态文件旁边。
//!
//! 解决三个具体问题：
//!
//! 1. **渲染后端选不对就打不开 / 花屏**。无 GPU 驱动的环境（虚拟机、精简版
//!    Windows、远程桌面）下 DX12 会退化成 WARP 软件光栅化，Vulkan / OpenGL 干脆
//!    没有适配器 —— 具体哪个能用只有到了那台机器上才知道。所以：按顺序试，
//!    下次直接用上次的结果；用户也可以在设置里手动锁定一个。
//! 2. **上一次启动没能跑起来**。写下「正在尝试 X」，跑起来之后才清掉。下次启动
//!    如果发现这个标记还在，说明 X 那次是崩在启动路上了 —— 自动把 X 降到最后再试
//!    别的。这样即使某个后端会让进程直接死掉，用户再点一次也能进得去，
//!    而不是永远卡在同一个坑里。
//! 3. **跑起来了，但挑中的是软件光栅化**。这一条最容易被漏掉：
//!    「能出帧」曾经就是全部判据，于是 `Auto` 挑中一个退化成 WARP 的适配器之后
//!    照样被记成 `last_good`，从此**每次启动都用它** —— 用户的原话是
//!    「默认的渲染引擎反而最卡，手动换一个倒好了」。而同一台机器上换个后端
//!    往往就有真实硬件适配器。所以软件渲染单独归一类（`slow`），不算「好」，
//!    下次启动自动换下一个；全试过仍是软件渲染才认命定下来，绝不无限轮换。
//!    见 [`resolve_after_success`]。
//!
//! 任何一步失败（目录建不了、文件读不出、JSON 坏了）都**只当作没有配置**继续走 ——
//! 这个模块的存在是为了让应用更容易打开，它自己绝不能成为打不开的理由。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// 与 `ViewportBuilder::with_app_id` 一致；同时决定 eframe 的状态目录位置。
pub const APP_ID: &str = "ferric";

/// 渲染后端选择。`Auto` = 交给 wgpu 自己挑（默认）。
///
/// `Glow` 是独立于 wgpu 的 OpenGL 渲染器（eframe::Renderer::Glow）——
/// 虚拟机 / 无 GPU 环境里它走系统的 OpenGL 软件实现，native 内存比 wgpu 回退
/// 到 WARP 小得多，作为这类环境的兑底。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Backend {
    #[default]
    Auto,
    Dx12,
    Vulkan,
    Gl,
    Glow,
    /// 纯 CPU 软渲染：不创建任何 GPU 上下文，进程内存最低。
    /// 永远能启动（不依赖驱动），代价是渲染在 CPU 上做、帧率较低。
    Soft,
}

impl Backend {
    /// 全部可选项（设置页按这个顺序展示）。
    ///
    /// `Soft` 排第一：它是唯一不碰 GPU API 的后端，内存最低（~150M vs wgpu 的
    /// ~640M），且任何机器（含无显卡的虚拟机）都能跑。ferric 是轻量工具，
    /// CPU 光栅化足够流畅；想要 GPU 加速的用户在设置里手动切 `Auto` 即可。
    pub const ALL: [Self; 6] = [
        Self::Soft,
        Self::Auto,
        Self::Dx12,
        Self::Vulkan,
        Self::Gl,
        Self::Glow,
    ];

    /// 传给 wgpu 的 `WGPU_BACKEND` 值；`Auto` 与 `Glow` 没有值 ——
    /// `Glow` 不走 wgpu，由入口直接选 eframe 的 glow 渲染器。
    ///
    /// 名字必须是 wgpu 认的那几个（见 `wgpu::Backends::from_comma_list`），
    /// 写错了 wgpu 只会 warn 一句然后当成空集合 —— 那等于一个后端都不许用。
    pub fn env_value(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Dx12 => Some("dx12"),
            Self::Vulkan => Some("vulkan"),
            Self::Gl => Some("gl"),
            Self::Glow => None,
            // 软渲染不走 wgpu，由入口直接走 ferric-soft-render。
            Self::Soft => None,
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
            Self::Glow => "Glow（OpenGL）",
            Self::Soft => "软渲染（CPU）",
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
    /// 试过、能跑起来，但**只拿到软件光栅化适配器**的后端。
    ///
    /// 为什么必须单独记一类：从前的自愈只有「能不能启动」一个判据，于是
    /// `Auto` 挑中一个退化成 WARP 的适配器之后，照样被记成 `last_good` ——
    /// 从此**每次启动都用它**，用户看到的就是「默认的渲染引擎反而最卡，
    /// 手动换一个倒好了」。可同一台机器上换个后端往往就有真实硬件适配器
    /// （DX12 没驱动会退化成 WARP，而 Vulkan / GL 未必）。
    ///
    /// 所以：软件渲染算「跑起来了，但不该就这么定下来」—— 排在没试过的之后、
    /// 起不来的之前，下次启动自动换下一个再看看。全都试过仍是软件渲染，
    /// 就认命定下来（见 [`resolve_after_success`]），绝不无限轮换。
    #[serde(default)]
    pub slow: Vec<Backend>,
    /// 上一次启动失败的原因（设置页展示，便于用户看见到底缺什么）。
    #[serde(default)]
    pub last_error: Option<String>,
    /// Windows/DX12：关闭 wgpu 的 frame-latency waitable object（等价于
    /// `WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT=none`）。
    /// 失焦期间该等待对象长时间不被 DWM 唤醒，是 Alt+Tab 切回瞬间卡一下的
    /// 主要嫌疑；设置里给开关是为了让用户两键完成 A/B 对比，不用敲 PowerShell。
    #[serde(default)]
    pub dx12_no_latency_wait: bool,
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
/// 2. 否则（没锁定）软渲染 `Soft` 排第一：它内存最低、任何机器都能跑，
///    是默认姿态；想要 GPU 加速的用户在设置里锁一个 wgpu 后端即可；
/// 3. 然后是上次成功的那个（若它不是上面那个，且没被降级）；
/// 4. 再按默认顺序逐个点名；
/// 5. 只拿到软件光栅化的（`slow`）降到没试过的之后 —— 能用，但还有更好的可找；
/// 6. 试过没跑起来的（`failed`）一律降到最后。
pub fn plan(cfg: &LaunchCfg) -> Vec<Backend> {
    fn push(order: &mut Vec<Backend>, b: Backend) {
        if !order.contains(&b) {
            order.push(b);
        }
    }
    /// 把 `demote` 里的项按原有先后挪到队尾。
    fn demote(order: &mut Vec<Backend>, demote: &[Backend]) {
        for b in demote.iter().copied() {
            if order.contains(&b) {
                order.retain(|x| *x != b);
                order.push(b);
            }
        }
    }
    let mut order: Vec<Backend> = Vec::with_capacity(Backend::ALL.len());
    if cfg.backend != Backend::Auto {
        push(&mut order, cfg.backend);
    } else {
        // 没锁定时软渲染优先：内存最低（~150M vs wgpu ~640M），任何机器都能跑。
        // 必须排在 last_good **之前** —— 否则老用户升级后仍被上次的 wgpu 记录压着，
        // 永远用不上软渲染（实测：升级后内存照样 600M+，就是栽在这）。
        push(&mut order, Backend::Soft);
    }
    if let Some(b) = cfg.last_good {
        push(&mut order, b);
    }
    for b in Backend::ALL {
        push(&mut order, b);
    }
    // 先降「只有软件渲染」的，再降「起不来」的 —— 两步的顺序决定了最终的相对位置：
    // 起不来的必须在最后，软件渲染的排它前面（软件渲染再慢也比打不开强）。
    //
    // 用户**锁定**的那个不参与降级：他可能就是要它（某些驱动下软件渲染反而画得对），
    // 「我选了 X 却被自动换成 Y」比慢更让人火大。起不来是另一回事 —— 那不是慢，
    // 是根本进不去，只能降。
    //
    // ⚠️ 判「有没有锁」必须先看 `cfg.backend != Auto`，不能直接拿 `b != cfg.backend`
    // 过滤：没锁时 `cfg.backend` 就是 `Auto`，那样写会把 slow 里的 `Auto` 一并放过 ——
    // 而 `Auto` 恰恰是最常进 slow 的那个（默认就先试它）。结果是它永不降级、
    // 每次启动照旧第一个用它，轮换根本开始不了（实测：连开五次全是 Auto，
    // slow 停在 ["Auto"] 不动，而横幅还在说「下次改用 DX12」）。
    let locked = (cfg.backend != Backend::Auto).then_some(cfg.backend);
    let slow: Vec<Backend> = cfg
        .slow
        .iter()
        .copied()
        .filter(|b| Some(*b) != locked)
        .collect();
    demote(&mut order, &slow);
    demote(&mut order, &cfg.failed);
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
    // DX12 帧等待对象开关。用户在外部显式设了环境变量就尊重外部值 ——
    // 环境变量是更明确的意图表达，配置只补缺省。
    if cfg.dx12_no_latency_wait
        && std::env::var_os("WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT").is_none()
    {
        std::env::set_var("WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT", "none");
        log("DX12 frame-latency waitable object 已按设置关闭（Alt+Tab 卡顿缓解）");
    }
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
    // 外部显式设了就一切照旧 —— 环境变量是比配置更明确的意图表达，
    // 而且它是**排障时唯一不用改配置就能做 A/B 的手段**。
    //
    // 这里原本在 `Auto` 分支无条件 `remove_var`，理由写的是「同一进程里我们会连试
    // 几个」—— 可 `main` 里写得很清楚：winit 全进程只允许一次事件循环，自愈是
    // **跨启动**的，同一进程根本不会试第二个。于是那行清除只剩副作用：
    // 把用户从命令行设的 WGPU_BACKEND 抹掉，`WGPU_BACKEND=gl ferric` 完全不起作用
    // （本次排障就栽在这儿：三种设法跑出来是同一个适配器）。
    if std::env::var_os("WGPU_BACKEND").is_some_and(|v| !v.is_empty()) {
        log("WGPU_BACKEND 已由外部指定，本次忽略配置里的后端选择");
        return;
    }
    if let Some(v) = backend.env_value() {
        std::env::set_var("WGPU_BACKEND", v);
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

/// 改「Alt+Tab 卡顿缓解（DX12）」这一项，返回落盘后的完整配置。
/// 与 [`set_backend`] 同样必须先重读磁盘再改，理由见彼处。
pub fn set_dx12_no_latency_wait(on: bool) -> LaunchCfg {
    match path() {
        Some(p) => {
            let mut cfg = load_from(&p);
            cfg.dx12_no_latency_wait = on;
            save_to(&p, &cfg);
            cfg
        }
        None => LaunchCfg {
            dx12_no_latency_wait: on,
            ..Default::default()
        },
    }
}

/// 进程内标记：UI 是否已经真的画出来过。
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 一次成功启动之后要让调用方知道的事。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// 用户锁定的后端被证明不可用，已改回「自动」（调用方应当提示）。
    pub lock_dropped: Option<Backend>,
    /// 本次只拿到软件光栅化，且**还有没试过的后端**，下次启动会自动改用它。
    /// 调用方应当把这件事告诉用户，并给一个「立即重启」的去处 ——
    /// 否则这个自愈要等到用户自己想起来重启才生效，而他正卡着。
    pub will_retry_with: Option<Backend>,
}

/// 应用已经稳定出帧了。把本次的结果落盘，并告诉调用方接下来该提示什么。
///
/// `software`：本次真正拿到的适配器是不是 CPU 软件光栅化（WARP / llvmpipe）。
///
/// # 「跑起来了」不等于「就用它了」
///
/// 从前这里只有一个判据 —— 能出帧就记成 `last_good`。于是 `Auto` 挑中一个
/// 退化成 WARP 的适配器之后就被永久定下来，用户的体验是「默认的渲染引擎最卡，
/// 手动换一个反而好」。而同一台机器上换个后端常常就有真实硬件适配器
/// （DX12 缺驱动会退化成 WARP，Vulkan / GL 未必）。
///
/// 所以软件渲染单独归一类（`slow`），不进 `last_good`，下次启动自动换下一个。
/// 全部后端都试过仍是软件渲染 → 这台机器就是没有硬件加速，认下最后这个，
/// 停止轮换（不然每次启动都在换后端，那是另一种毛病）。
///
/// 由 `FerricApp` 在头几帧之后调用一次（只有第一次生效）。
pub fn mark_running(software: bool) -> Outcome {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return Outcome::default();
    }
    let mut cfg = load();
    let used = cfg.pending.unwrap_or_else(|| {
        std::env::var("WGPU_BACKEND")
            .ok()
            .map_or(Backend::Auto, |v| Backend::from_env_value(&v))
    });
    let out = resolve_after_success(&mut cfg, used, software);
    cfg.pending = None;
    save(&cfg);
    out
}

/// [`mark_running`] 的纯逻辑部分（可测：不碰磁盘、不碰环境变量）。
///
/// 两件事：① 用户锁是否已被证明不可用 → 放弃锁回到自动；
/// ② 本次是不是只拿到软件渲染 → 决定记 `last_good` 还是记 `slow`。
fn resolve_after_success(cfg: &mut LaunchCfg, used: Backend, software: bool) -> Outcome {
    let mut out = Outcome::default();

    // ① 锁不可满足就得放弃 —— 否则「锁了台机器上没有的后端」会变成隔次启动必失败的
    //    死循环：失败 → 兜底成功清黑名单 → 又试锁 → 又失败……（实测 startup.log 如此）。
    if cfg.backend != Backend::Auto && cfg.backend != used && cfg.failed.contains(&cfg.backend) {
        out.lock_dropped = Some(cfg.backend);
        cfg.last_error = Some(format!(
            "锁定的 {} 在本机不可用，已改回「自动」",
            cfg.backend.label()
        ));
        cfg.backend = Backend::Auto;
    }

    if !software {
        // 拿到硬件加速 —— 这才叫「好」。两个黑名单一并清掉：能拿到硬件加速说明
        // 环境确实变好了（装了驱动 / 开了 3D 加速），之前被判过「起不来」或
        // 「只有软件渲染」的后端都值得重新给机会。
        cfg.last_good = Some(used);
        cfg.slow.clear();
        cfg.failed.clear();
        if out.lock_dropped.is_none() {
            cfg.last_error = None;
        }
        return out;
    }

    // —— 软件渲染。记一笔，看还有没有没试过的。
    //
    // ⚠️ 这条路径上**绝不能**清 `failed`。清了的话，轮换会一头撞回那些在本机
    // 根本起不来的后端上 —— 实测（Linux 上没有 DX12）：轮换刚做出来时是
    //   自动 → DX12(打不开) → Vulkan → DX12(打不开) → OpenGL → DX12(打不开)
    // 也就是**隔一次启动应用就打不开**，比原来的卡顿糟得多。
    // 「环境会变好」的重试机会留给拿到硬件加速的那条分支就够了。
    if !cfg.slow.contains(&used) {
        cfg.slow.push(used);
    }
    // 用户**锁定**了这个后端就不再替他轮换：他的选择优先于我们的判断。
    let locked = cfg.backend != Backend::Auto;
    let untried: Option<Backend> = if locked {
        None
    } else {
        // 内存优先：拿到软件渲染（无 GPU）时，下一个先试「纯软渲染 Soft」——
        // 它不建任何 GPU 上下文，进程内存 ~150M；而 DX12 / Vulkan / Gl 这些 wgpu
        // 后端在无 GPU 环境都会退化成 WARP，内存同样 ~640M，挨个试它们只是白试。
        // 只有 Soft 也试过（或起不来）了，才退回「逐个找真实 GPU」的老路。
        if !cfg.slow.contains(&Backend::Soft) && !cfg.failed.contains(&Backend::Soft) {
            Some(Backend::Soft)
        } else {
            // 起不来的（failed）同样要跳过 —— 它们不是「没试过」，是试过进不去。
            plan(cfg)
                .into_iter()
                .find(|b| !cfg.slow.contains(b) && !cfg.failed.contains(b))
        }
    };
    match untried {
        Some(next) => {
            // 先别定下 last_good —— 定了下次启动第一个又是它，轮换就永远开始不了。
            out.will_retry_with = Some(next);
            cfg.last_error = Some(format!(
                "{} 只拿到软件渲染（无 GPU 加速），下次启动将改用 {}",
                used.label(),
                next.label()
            ));
        }
        None => {
            // 全试过了（或用户锁死了），这台机器就是没有硬件加速。认下它，停止轮换。
            cfg.last_good = Some(used);
            cfg.last_error = Some(
                "所有渲染后端在本机都只有软件渲染（无 GPU 加速）—— \
                 虚拟机请开启 3D 加速，物理机请安装显卡驱动"
                    .to_owned(),
            );
        }
    }
    out
}

/// 本次启动是否已经成功出帧。
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// 重启本应用：拉起一份新的自己，然后请调用方关掉当前窗口。
///
/// 渲染后端只能在建窗**之前**决定（`WGPU_BACKEND` 是构造 `NativeOptions` 时读的），
/// 所以换后端必然要重启。没有这个按钮的话，「重启后生效」就是把活儿丢回给用户：
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

    /// Alt+Tab 卡顿缓解开关要能落盘并读回；老 launch.json 没有该字段时按关处理。
    #[test]
    fn dx12_wait_flag_round_trips_and_defaults_off() {
        let dir = std::env::temp_dir().join(format!("ferric-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("launch.json");

        // 老配置（无该字段）→ 默认关
        std::fs::write(&p, r#"{"backend":"Gl"}"#).unwrap();
        let cfg = load_from(&p);
        assert!(
            !cfg.dx12_no_latency_wait,
            "缺省必须是关，别替用户改呈现行为"
        );
        assert_eq!(cfg.backend, Backend::Gl, "旧字段不能因新字段而丢");

        // 开 → 存 → 读回，且不动其它字段
        let mut cfg = cfg;
        cfg.dx12_no_latency_wait = true;
        save_to(&p, &cfg);
        let back = load_from(&p);
        assert!(back.dx12_no_latency_wait);
        assert_eq!(back.backend, Backend::Gl);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 锁定的后端起不来、靠兜底跑起来的 → 锁必须改回「自动」并说明原因。
    /// 否则成功启动清掉黑名单后，下次又会先试那个锁 —— 隔次启动必失败的死循环。
    #[test]
    fn an_unsatisfiable_lock_falls_back_to_auto() {
        let mut cfg = LaunchCfg {
            backend: Backend::Gl,
            failed: vec![Backend::Gl],
            ..Default::default()
        };
        let out = resolve_after_success(&mut cfg, Backend::Auto, false);
        assert_eq!(out.lock_dropped, Some(Backend::Gl));
        assert_eq!(cfg.backend, Backend::Auto, "锁没被放弃，死循环还在");
        let msg = cfg.last_error.as_deref().unwrap_or("");
        assert!(msg.contains("OpenGL"), "原因没写给用户看：{msg:?}");
    }

    /// 锁定的后端本次**自己**跑起来了 → 锁保留（哪怕之前进过黑名单）。
    #[test]
    fn a_lock_that_started_fine_is_kept() {
        let mut cfg = LaunchCfg {
            backend: Backend::Gl,
            failed: vec![Backend::Gl], // 上上次失败过，这次成功
            ..Default::default()
        };
        assert_eq!(
            resolve_after_success(&mut cfg, Backend::Gl, false).lock_dropped,
            None
        );
        assert_eq!(cfg.backend, Backend::Gl, "自己跑起来的锁不该被动");
    }

    /// 没锁（自动）或锁没进过黑名单 → 一切不动。
    #[test]
    fn auto_mode_and_healthy_locks_are_untouched() {
        let mut auto = LaunchCfg::default();
        assert_eq!(
            resolve_after_success(&mut auto, Backend::Dx12, false).lock_dropped,
            None
        );

        let mut healthy = LaunchCfg {
            backend: Backend::Vulkan,
            ..Default::default()
        };
        // 本次因为别的原因用了 Auto（比如首次 pending 逻辑），但 Vulkan 没失败记录
        assert_eq!(
            resolve_after_success(&mut healthy, Backend::Auto, false).lock_dropped,
            None
        );
        assert_eq!(healthy.backend, Backend::Vulkan);
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
                slow: Vec::new(),
                last_error: None,
                dx12_no_latency_wait: false,
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

    // ==== 「能跑起来」≠「就用它了」：软件渲染的自愈 ====

    /// 拿到软件渲染时**不能**记成 last_good —— 记了下次启动第一个又是它，
    /// 轮换永远开始不了，用户就永远卡着。这正是「默认的渲染引擎最卡」的成因。
    #[test]
    fn a_software_only_backend_is_not_accepted_as_good() {
        let mut cfg = LaunchCfg::default();
        let out = resolve_after_success(&mut cfg, Backend::Auto, true);
        assert_eq!(cfg.last_good, None, "软件渲染被当成好结果记下来了");
        assert!(cfg.slow.contains(&Backend::Auto), "没记进 slow");
        assert!(out.will_retry_with.is_some(), "没给出下次要试的后端");
        assert_ne!(
            out.will_retry_with,
            Some(Backend::Auto),
            "下次还试同一个 = 原地打转"
        );
    }

    /// 拿到硬件加速才算数：记 last_good，并把 slow 清空 ——
    /// 装了驱动 / 虚拟机开了 3D 加速，之前判过慢的后端值得重新给机会。
    #[test]
    fn hardware_acceleration_is_what_gets_remembered() {
        let mut cfg = LaunchCfg {
            slow: vec![Backend::Dx12, Backend::Auto],
            failed: vec![Backend::Gl],
            ..Default::default()
        };
        let out = resolve_after_success(&mut cfg, Backend::Vulkan, false);
        assert_eq!(cfg.last_good, Some(Backend::Vulkan));
        assert!(cfg.slow.is_empty(), "环境变好了，slow 该清空");
        assert!(cfg.failed.is_empty(), "成功启动后黑名单该清空");
        assert_eq!(out.will_retry_with, None, "已经good了还提示重启就是骚扰");
        assert_eq!(cfg.last_error, None);
    }

    /// **绝不无限轮换**：每个后端都试过、全是软件渲染 → 认下最后那个，
    /// 不再要求重启。否则这台机器每次启动都在换后端，那是另一种毛病。
    #[test]
    fn a_machine_with_no_gpu_at_all_settles_instead_of_rotating_forever() {
        let mut cfg = LaunchCfg::default();
        let mut used = Backend::Auto;
        // 模拟连续启动：每次都只拿到软件渲染
        for i in 0..Backend::ALL.len() {
            let out = resolve_after_success(&mut cfg, used, true);
            match out.will_retry_with {
                Some(next) => {
                    assert!(i < Backend::ALL.len() - 1, "第 {i} 轮还在要求换，该收敛了");
                    used = next;
                }
                None => {
                    assert_eq!(i, Backend::ALL.len() - 1, "过早停止轮换：第 {i} 轮");
                    break;
                }
            }
        }
        assert_eq!(cfg.slow.len(), Backend::ALL.len(), "没把每个后端都试到");
        assert_eq!(cfg.last_good, Some(used), "认命之后要记下来，别再换");
        // 再来一次也不该继续要求重启
        assert_eq!(
            resolve_after_success(&mut cfg, used, true).will_retry_with,
            None,
            "已经认命了还在要求重启"
        );
        let msg = cfg.last_error.as_deref().unwrap_or("");
        assert!(
            msg.contains("3D 加速") || msg.contains("驱动"),
            "没告诉用户真正该做什么：{msg:?}"
        );
    }

    /// 轮换**绝不能撞回那些在本机根本起不来的后端**。
    ///
    /// 这是加轮换时实测抓到的回归：软件渲染那条路径上原本也跟着清了 `failed`，
    /// 于是在没有 DX12 的机器上跑出了
    ///   自动 → DX12(打不开) → Vulkan → DX12(打不开) → OpenGL → DX12(打不开)
    /// —— **隔一次启动应用就打不开**，比原来的卡顿糟得多。
    #[test]
    fn rotation_never_walks_back_into_a_backend_that_cannot_start() {
        let mut cfg = LaunchCfg {
            failed: vec![Backend::Dx12], // 上次试 DX12 没能出帧
            slow: vec![Backend::Auto],
            ..Default::default()
        };
        let out = resolve_after_success(&mut cfg, Backend::Vulkan, true);
        assert_ne!(
            out.will_retry_with,
            Some(Backend::Dx12),
            "又把起不来的后端排成了下一个"
        );
        assert!(
            cfg.failed.contains(&Backend::Dx12),
            "软件渲染路径上把 failed 清了 —— 下次启动就会再撞一次墙"
        );
    }

    /// 但拿到硬件加速时 `failed` 该清空：那才是「环境真的变好了」的信号
    /// （装了驱动 / 虚拟机开了 3D 加速），值得让之前起不来的后端重新有机会。
    #[test]
    fn hardware_success_reopens_the_blacklist() {
        let mut cfg = LaunchCfg {
            failed: vec![Backend::Dx12],
            slow: vec![Backend::Auto],
            ..Default::default()
        };
        resolve_after_success(&mut cfg, Backend::Vulkan, false);
        assert!(
            cfg.failed.is_empty(),
            "拿到硬件加速后没给起不来的后端翻身机会"
        );
        assert!(cfg.slow.is_empty());
    }

    /// 用户**锁定**了某个后端，就算它只有软件渲染也不替他换 ——
    /// 「我选了 X 却被自动换成 Y」比慢更让人火大。
    #[test]
    fn a_users_explicit_lock_is_never_auto_rotated() {
        let mut cfg = LaunchCfg {
            backend: Backend::Gl,
            ..Default::default()
        };
        let out = resolve_after_success(&mut cfg, Backend::Gl, true);
        assert_eq!(out.will_retry_with, None, "锁定的后端被自动换掉了");
        assert_eq!(cfg.backend, Backend::Gl, "锁不该被动");
        assert_eq!(cfg.last_good, Some(Backend::Gl), "尊重锁定就要认下它");
        // 排序里也不能把锁定项降下去
        assert_eq!(
            plan(&cfg).first(),
            Some(&Backend::Gl),
            "锁定项被 slow 降级了"
        );
    }

    /// 没锁定时（`backend == Auto`），slow 里的 `Auto` 也必须照降。
    ///
    /// 这条是实测抓出来的回归：原先的过滤写成 `b != cfg.backend`，而没锁时
    /// `cfg.backend` 正是 `Auto`，于是最常进 slow 的那个永远不降级 ——
    /// 连开五次全在用 Auto、slow 停在 ["Auto"] 不动，轮换等于没做。
    #[test]
    fn auto_gets_demoted_too_when_nothing_is_locked() {
        let cfg = LaunchCfg {
            slow: vec![Backend::Auto],
            ..Default::default()
        };
        let p = plan(&cfg);
        assert_ne!(
            p.first(),
            Some(&Backend::Auto),
            "已知只有软件渲染的 Auto 还排第一 —— 轮换开始不了：{p:?}"
        );
        assert_eq!(p.last(), Some(&Backend::Auto), "Auto 该被排到最后：{p:?}");
    }

    /// 走一遍**真实**的跨启动循环（begin 落 pending → 成功后 resolve），
    /// 确认每次启动真的换了一个后端，而不是嘴上说换、实际原地打转。
    #[test]
    fn rotation_actually_advances_across_restarts() {
        let mut cfg = LaunchCfg::default();
        let mut used_each_time = Vec::new();
        for _ in 0..Backend::ALL.len() {
            // begin() 的纯逻辑部分：挑计划首项
            let b = plan(&cfg).first().copied().unwrap();
            used_each_time.push(b);
            // 这次也只拿到软件渲染
            resolve_after_success(&mut cfg, b, true);
        }
        let uniq: std::collections::HashSet<_> = used_each_time.iter().collect();
        assert_eq!(
            uniq.len(),
            Backend::ALL.len(),
            "跨启动没有真的逐个换过去：{used_each_time:?}"
        );
    }

    /// slow 的降级只是「往后排」，不是拉黑：起不来的必须排在它后面 ——
    /// 软件渲染再慢也比打不开强。
    #[test]
    fn slow_is_demoted_but_still_ranks_above_broken() {
        let cfg = LaunchCfg {
            slow: vec![Backend::Dx12],
            failed: vec![Backend::Vulkan],
            ..Default::default()
        };
        let p = plan(&cfg);
        let slow_at = p.iter().position(|b| *b == Backend::Dx12).unwrap();
        let bad_at = p.iter().position(|b| *b == Backend::Vulkan).unwrap();
        assert!(slow_at < bad_at, "「慢」被排到了「打不开」后面：{p:?}");
        assert!(
            p.contains(&Backend::Dx12),
            "slow 不是拉黑，不能从计划里消失"
        );
        // 没试过的要排在慢的前面
        let fresh_at = p.iter().position(|b| *b == Backend::Gl).unwrap();
        assert!(fresh_at < slow_at, "没试过的该优先于已知慢的：{p:?}");
    }

    /// 外部显式设的 WGPU_BACKEND 必须被尊重 —— 那是排障时唯一不改配置就能做
    /// A/B 的手段。此前 `Auto` 分支无条件 remove_var，把它抹掉了。
    #[test]
    fn an_externally_set_backend_env_var_wins() {
        // 这个测试要改进程环境变量，与其它用到 WGPU_BACKEND 的测试互斥；
        // 本文件里只有它碰这个变量。
        std::env::set_var("WGPU_BACKEND", "gl");
        apply(Backend::Dx12);
        assert_eq!(
            std::env::var("WGPU_BACKEND").ok().as_deref(),
            Some("gl"),
            "外部指定的后端被配置覆盖了"
        );
        apply(Backend::Auto);
        assert_eq!(
            std::env::var("WGPU_BACKEND").ok().as_deref(),
            Some("gl"),
            "「自动」把外部指定的后端清掉了 —— WGPU_BACKEND=gl ferric 会完全失效"
        );
        std::env::remove_var("WGPU_BACKEND");
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
