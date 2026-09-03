//! 应用外壳：状态、布局、路由、持久化。

use crate::chrome::{self, TITLE_BAR_HEIGHT};
use crate::fonts::{UI_BOLD, UI_SEMIBOLD};
use crate::theme::Theme;
use crate::tool::{Shared, Tool};
use crate::{fonts, icons, views, widgets};
use egui::{
    vec2, Align, Align2, CentralPanel, Color32, CornerRadius, FontFamily, FontId, Frame, Key,
    Layout, Margin, Panel, RichText, ScrollArea, Sense, Stroke,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const APP_NAME: &str = "Ferric";

const RAIL_DEFAULT: f32 = 264.0;
const RAIL_MIN: f32 = 196.0;
const RAIL_MAX: f32 = 460.0;

/// 主题模式：默认跟随系统深浅色，也可手动锁定。
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize)]
struct Persist {
    dark: bool, // 最近一次生效的深浅色（启动首帧兜底，避免闪白/闪黑）
    #[serde(default)]
    theme_mode: Option<ThemeMode>, // 旧版本数据无此字段 → None，迁移见 new()
    rail_width: f32,
    favorites: Vec<String>,
    active_id: String,
    #[serde(default)]
    drafts: HashMap<String, String>,
    #[serde(default)]
    lang: crate::tool::Lang,
    /// 自定义更新服务器（地址 + 公钥**作为整体**存取，禁止只改其一）。
    /// None = 用编译期烘入的默认服务器。
    ///
    /// ⚠️ 这个 ron 文件是当前用户可写的，任何同用户进程都能静默改写它 ——
    /// 所以一旦不是内置服务器，自动安装会被禁用（降级为仅通知），且 UI 挂持久告警。
    #[serde(default)]
    server: Option<crate::net::ServerProfile>,
    /// 全局界面字号缩放（1.0 = 标准）。
    #[serde(default = "default_ui_scale")]
    ui_scale: f32,
    /// 代码编辑区排版（字号 / 字重 / 行距）。
    #[serde(default)]
    code_font: crate::widgets::FontCfg,
    /// 自动检查更新并在后台下载。默认开 —— 更新只有及时装上才有意义。
    #[serde(default = "default_true")]
    auto_update: bool,
    /// 旧字段：是否使用演示数据（`None` 自动 / `false` 服务器 / `true` 演示）。
    /// 已被 `source_pref` 取代，保留是为了**双向兼容** —— 读旧文件时迁移，
    /// 写回时也顺手填上，装回旧版本的用户不至于把设置丢光。
    #[serde(default)]
    mock_source: Option<bool>,
    /// 数据源选择（自动 / 服务器 / GitHub / 演示）。旧文件里没有 → 从 `mock_source` 迁移。
    #[serde(default)]
    source_pref: Option<crate::source::SourcePref>,
    /// 自定义 GitHub 更新源（`owner/repo`）。None = 用编译期烘入的那个。
    ///
    /// ⚠️ 与自定义服务器同样的道理：这个 ron 文件当前用户可写，所以一旦不是内置
    /// 仓库，自动下载与自动安装都会被禁用（降级为仅提示）。
    #[serde(default)]
    github_repo: Option<String>,
    /// 上次成功检查更新的 Unix 时间戳（秒）。跨启动节流用 ——
    /// 只放在内存里的话，开十次应用就查十次。
    #[serde(default)]
    last_update_check: Option<i64>,
}

fn default_true() -> bool {
    true
}

fn default_ui_scale() -> f32 {
    1.0
}

impl Default for Persist {
    fn default() -> Self {
        Self {
            dark: false,
            theme_mode: Some(ThemeMode::System),
            rail_width: RAIL_DEFAULT,
            favorites: Vec::new(),
            active_id: "json".to_owned(),
            drafts: HashMap::new(),
            lang: crate::tool::Lang::default(),
            server: None,
            ui_scale: 1.0,
            code_font: Default::default(),
            auto_update: true,
            mock_source: None,
            source_pref: None,
            github_repo: None,
            last_update_check: None,
        }
    }
}

/// 当前 Unix 时间戳（秒）。系统时钟异常时回落到 0（= 「很久没查过」）。
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 设置窗自绘的标题栏：与主窗同一套语言（无边框 + 拖拽 + 关闭），
/// 拖动交给系统，因此可以拖到屏幕任何位置。
fn settings_window_chrome(root_ui: &mut egui::Ui, theme: &Theme, open: &mut bool) {
    let ctx = root_ui.ctx().clone();
    Panel::top("settings-title")
        .exact_size(44.0)
        .frame(Frame::NONE.fill(theme.bg))
        .show_separator_line(false)
        .show(root_ui, |ui| {
            let rect = ui.max_rect();
            // 背景拖拽区（按钮随后覆盖其上）
            let drag = ui.interact(
                rect,
                egui::Id::new("settings-title-drag"),
                Sense::click_and_drag(),
            );
            if drag.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.painter().text(
                rect.left_center() + vec2(18.0, 0.0),
                Align2::LEFT_CENTER,
                "设置",
                FontId::new(15.0, FontFamily::Name(UI_SEMIBOLD.into())),
                theme.fg,
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(6.0);
                    let (r, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(r, CornerRadius::same(8), theme.border);
                    }
                    ui.painter().text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        icons::X,
                        FontId::new(13.0, icons::family()),
                        if resp.hovered() {
                            theme.fg
                        } else {
                            theme.muted
                        },
                    );
                    if resp.clicked() {
                        *open = false;
                    }
                });
            });
            ui.painter().hline(
                rect.x_range(),
                rect.bottom(),
                Stroke::new(1.0_f32, theme.border),
            );
        });
}

/// 设置窗左侧的分类。
///
/// 设置从「一长列纵向堆叠」改成「左分类 + 右内容」，与主界面同一套结构：
/// 六个区块挤在一列里时，改个字号要先滚过渲染后端和更新设置，找东西全靠翻；
/// 分好类之后每屏只呈现一类，标签与控件也能并排放下，不必再挤成两行。
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SettingsTab {
    #[default]
    Appearance,
    Font,
    Renderer,
    Update,
    Data,
    About,
}

impl SettingsTab {
    /// 分类栏的顺序与图标。放在一处，免得导航和内容分发两边各写一份而走散。
    const ALL: [(SettingsTab, char, &'static str); 6] = [
        (SettingsTab::Appearance, icons::SUN, "外观"),
        (SettingsTab::Font, icons::TYPE_ICON, "字体"),
        (SettingsTab::Renderer, icons::SQUARE, "渲染"),
        (SettingsTab::Update, icons::REFRESH_CW, "更新"),
        (SettingsTab::Data, icons::DATABASE, "数据"),
        (SettingsTab::About, icons::INFO, "关于"),
    ];
}

pub struct FerricApp {
    tools: Vec<Box<dyn Tool>>,
    active: usize,
    mode: ThemeMode,
    dark: bool, // 当前生效的深浅色（mode 为 System 时由系统主题解析而来）
    rail_width: f32,
    favorites: HashSet<String>,
    rail_filter: String,
    focus_search: bool,
    settings_open: bool,
    /// 自定义更新服务器；None 表示用内置的
    server_override: Option<crate::net::ServerProfile>,
    /// 设置页里正在编辑的服务器草稿（未点「应用」前不生效）
    server_draft: Option<(String, String)>,
    /// 「核对指纹」输入框。**故意不持久化** —— 它是一次性的核对动作，
    /// 存下来只会让下次打开设置页时显示一个早已过期的「一致」。
    fingerprint_input: String,
    updater: crate::updater::Updater,
    shared: Shared,
    /// 调试自拍的帧计数（仅在 `FERRIC_SCREENSHOT` 下有意义，见 `debug_screenshot`）。
    shot_frames: u32,
    /// 全局界面字号缩放（1.0 = 标准）。
    ui_scale: f32,
    /// 已经生效的缩放值；与 `ui_scale` 不一致时才重写样式，避免每帧克隆 Style。
    applied_ui_scale: Option<f32>,
    /// 待把设置窗调到最前（点「设置」时置位，下一帧处理后清除）。
    settings_raise: bool,
    /// 「调到最前」的状态机，见 [`SettingsRaise`]。
    raise: SettingsRaise,
    /// 已经画过的帧数（只数到 [`STABLE_FRAMES`] 为止），用于确认本次启动是成功的。
    frames: u32,
    /// 启动期配置（渲染后端等）。改动即刻落盘，下次启动生效。
    launch_cfg: crate::launch::LaunchCfg,
    /// 自动检查更新并后台下载。
    auto_update: bool,
    /// 数据源选择（自动 / 服务器 / GitHub / 演示）。
    source_pref: crate::source::SourcePref,
    /// 自定义 GitHub 更新源；None 表示用编译期烘入的那个。
    github_override: Option<crate::github::GithubSource>,
    /// 设置页里正在编辑的 GitHub 仓库草稿（未点「应用」前不生效）。
    github_draft: Option<String>,
    /// 上次成功检查更新的 Unix 时间戳（秒）。
    last_update_check: Option<i64>,
    /// 内置工具的数量。插件工具一律追加在它们之后，热加载时按这个位置截断 ——
    /// 这样内置工具（连同它们的界面状态）不会被重建，只有插件那截换掉。
    builtin_tools: usize,
    /// 上一帧是否持有系统焦点（Alt+Tab 切回时的旧帧频闪缓解，见 `present_hygiene`）。
    was_focused: bool,
    /// 窗口外沿左上角在上一帧的位置（拖动窗口检测）。
    last_outer_pos: Option<egui::Pos2>,
    /// 最近一次检测到窗口移动的时刻。
    last_moved_at: Option<std::time::Instant>,
    /// 当前表面是否处于「拖动期免等垂直同步」模式（避免每帧重复重配表面）。
    fast_present: bool,
    /// 上一帧开始的时刻，仅用于软渲染下的帧率上限（见 [`FerricApp::throttle_soft_render`]）。
    last_frame_at: Option<std::time::Instant>,
    /// 设置窗当前选中的分类（不持久化：每次打开都从「外观」进）。
    settings_tab: SettingsTab,
    /// 本次运行实际拿到的图形适配器描述（「后端 · 显卡名」），设置页展示。
    /// None = 非 wgpu 渲染路径（理论上不会发生，防御性处理）。
    gpu_desc: Option<String>,
    /// 实际适配器是 CPU 软件光栅化（WARP / llvmpipe）—— 虚拟机与无驱动环境。
    /// 设置页据此提示「画面糊 / 卡的根源在这，换后端或开 3D 加速」。
    gpu_software: bool,
    /// 本次只拿到软件渲染，自愈已为下次启动排好这个后端 —— 顶栏据此显示
    /// 一条「立即重启试试」的横幅。`None` = 没这回事（绝大多数机器）。
    slow_render_retry: Option<crate::launch::Backend>,
    /// 用户在设置里改了渲染后端、尚未重启生效 —— 设置页据此给出「立即重启」。
    pending_restart: Option<crate::launch::Backend>,
    /// 用户点了「立即重启」，等本帧画完由外壳执行（见 `do_restart`）。
    want_restart: bool,
    /// 更新就绪后自动弹出的更新框是否打开（「稍后」/ Esc / 点背景关闭）。
    update_dialog_open: bool,
    /// 软渲染后端注入的持久化存储。eframe 路径为 `None`（走 `frame.storage_mut()`），
    /// 软渲染路径由 [`FerricApp::new_soft`] 注入，[`FerricApp::save_soft`] 用它落盘。
    soft_storage: Option<Box<dyn eframe::Storage>>,
    /// 30 秒内存采样的状态机：None = 不在工作。详见 `crate::mem`。
    mem_recorder: Option<crate::mem::MemoryRecorder>,
}

/// 连续画满这么多帧就认定「这次启动是好的」，把当前渲染后端记成 last_good。
///
/// 不在第 1 帧就记：wgpu 的适配器是有了，但首次真正提交画面时才可能暴露问题
/// （设备丢失、表面配置不受支持）。多等两帧，代价是零，换来的是这个标记名副其实。
const STABLE_FRAMES: u32 = 3;

/// 等 `Focus` 生效的帧数；到期还没拿到焦点就走重建兜底。
/// 8 帧 ≈ 130ms @60fps：够窗口管理器响应，又不至于让人察觉延迟。
const RAISE_WAIT_FRAMES: u8 = 8;

/// 「销毁窗口再建一个」这条兜底，是否是本平台唯一能把窗口提到前面的办法。
///
/// 只有 Wayland 是（合成器不让客户端抢焦点，`Focus` 与 `AlwaysOnTop` 都是空操作）。
/// X11 / Windows / macOS 的 `Focus` 是有效的 —— 在那些平台上重建窗口纯粹是**白闪一下**，
/// 而且 Windows 的 `SetForegroundWindow` 有它自己的节流规则，偶尔慢一拍就会被
/// 「等 8 帧没焦点」误判成失败，于是每点一次设置就闪一次。
fn recreate_is_the_only_raise() -> bool {
    // winit 在两个后端都编进来时优先选 Wayland，因此以 WAYLAND_DISPLAY 是否存在为准。
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// 把设置窗「调到最前」的状态机。
///
/// 设置窗是独立系统窗口，会被别的窗口盖住。想把它翻到前面，各平台能用的手段不一样：
///
/// - `ViewportCommand::Focus` 在 X11 / Windows / macOS 上有效；
/// - **Wayland 上它是文档明说的空操作**（合成器防止应用抢焦点），
///   `WindowLevel::AlwaysOnTop` 在 winit 的 Wayland 后端同样是个空函数，
///   父子窗口关系也没透传出来 —— 客户端根本没有合法途径把自己提到前面。
///
/// 那里唯一有效的办法是**把窗口销毁再建一个**：新映射的窗口合成器会呈现在最前。
/// 代价是闪一下、位置由合成器重定，所以只在 `Focus` 确实没生效时才回落。
/// 判定方式不做平台检测 —— 发完命令等几帧看有没有拿到焦点，谁生效就用谁。
#[derive(Default)]
struct SettingsRaise {
    /// 发过 `Focus` 后还在等的剩余帧数。
    wait: u8,
    /// 还需跳过不渲染的帧数（跳过 = 销毁窗口）。
    reopen: u8,
    /// 上一帧窗口是否持有焦点。
    focused: bool,
}

impl SettingsRaise {
    /// 收到一次置顶请求。已经在前台就什么都不做 —— 重建一个已经看得见的窗口只是白闪。
    fn request(&mut self) {
        if !self.focused {
            self.wait = RAISE_WAIT_FRAMES;
        }
    }

    /// 是否正在处理置顶请求（需要持续出帧才能把计数推下去）。
    fn busy(&self) -> bool {
        self.wait > 0 || self.reopen > 0
    }

    /// 每帧推进一次。返回 `true` 表示这一帧**不要渲染**设置视口（即销毁它）。
    ///
    /// `can_recreate` = 本平台是否需要「销毁重建」这条兜底（见
    /// [`recreate_is_the_only_raise`]）。为 false 时等超时只是放弃，不闪。
    fn tick(&mut self, can_recreate: bool) -> bool {
        if self.wait > 0 {
            if self.focused {
                self.wait = 0; // Focus 生效了，不必重建
            } else {
                self.wait -= 1;
                if self.wait == 0 && can_recreate {
                    self.reopen = 2; // 等超时 → 重建窗口
                }
            }
        }
        if self.reopen > 0 {
            self.reopen -= 1;
            self.focused = false; // 窗口这一帧不存在，谈不上有焦点
            return true;
        }
        false
    }
}

impl FerricApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let persist: Persist = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();

        // 记下真实拿到的适配器：设置页「渲染后端」区块要展示「现在实际用的是什么」。
        // 用户切后端做 A/B 对比时，没有这行就无从确认切换是否真的生效。
        //
        // `FERRIC_SOFT_RENDER=1` 强制按软件渲染处理。虚拟机 / 远程桌面的虚拟 GPU
        // 常被 wgpu 报告成 IntegratedGpu（device_type != Cpu），于是整套软渲染自适应
        // （关阴影/动画/羽化 + 帧率封顶）一个都不生效 —— 悬停闪动、拖动拖影多半源于此。
        // 无 GPU 环境下设这个变量，等同于明确告诉应用「别把虚拟 GPU 当真 GPU」。
        let force_soft = std::env::var_os("FERRIC_SOFT_RENDER").is_some();
        let (gpu_desc, gpu_software) = match cc.wgpu_render_state.as_ref() {
            Some(rs) => {
                let info = rs.adapter.get_info();
                let backend = match info.backend {
                    eframe::wgpu::Backend::Dx12 => "DX12",
                    eframe::wgpu::Backend::Vulkan => "Vulkan",
                    eframe::wgpu::Backend::Gl => "OpenGL",
                    eframe::wgpu::Backend::Metal => "Metal",
                    _ => "其他",
                };
                let sw = info.device_type == eframe::wgpu::DeviceType::Cpu || force_soft;
                (Some(format!("{backend} · {}", info.name)), sw)
            }
            // glow 渲染器没有 wgpu 的适配器信息。它主要是给虚拟机 / 无 GPU 环境
            // 作兑底的，默认按软件渲染处理（关动画/阴影/羽化 + 帧率封顶）。
            None => (Some("Glow（OpenGL）".to_owned()), true),
        };

        Self::build(&cc.egui_ctx, persist, gpu_desc, gpu_software)
    }

    /// 软渲染入口：不经过 eframe 的 `CreationContext` / wgpu 渲染栈，持久化存储
    /// 由调用方（软渲染后端）直接注入。界面逻辑与 [`FerricApp::new`] 完全一致，
    /// 只是 `gpu_software` 恒为真（纯 CPU 光栅化）。
    pub fn new_soft(ctx: &egui::Context, storage: Option<Box<dyn eframe::Storage>>) -> Self {
        let persist: Persist = storage
            .as_ref()
            .and_then(|s| eframe::get_value(s.as_ref(), eframe::APP_KEY))
            .unwrap_or_default();
        let gpu_desc = Some("CPU 软渲染".to_owned());
        let mut slf = Self::build(ctx, persist, gpu_desc, true);
        slf.soft_storage = storage;
        slf
    }

    /// 构造 [`FerricApp`] 的公共部分：装字体、算主题、建工具与共享状态。
    /// 两条入口（[`FerricApp::new`] / [`FerricApp::new_soft`]）汇合到这里。
    fn build(
        ctx: &egui::Context,
        persist: Persist,
        gpu_desc: Option<String>,
        gpu_software: bool,
    ) -> Self {
        let has_cjk = fonts::install_fonts(ctx);

        // 迁移：旧数据没有 theme_mode，一律改为跟随系统（旧版的 dark 只作首帧兜底）。
        let mode = persist.theme_mode.unwrap_or(ThemeMode::System);
        // 启动首帧系统主题可能尚未上报（system_theme() 为 None），
        // 先用上次生效的深浅色兜底，进入 update() 后每帧与系统同步。
        let dark = match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => ctx
                .system_theme()
                .map_or(persist.dark, |t| t == egui::Theme::Dark),
        };
        let theme = Theme::from_dark(dark);
        theme.apply(ctx);

        // 同一行写进 startup.log：排「画面糊 / 卡」这类问题时，
        // 「实际用了哪块适配器」是第一个要回答的问题。
        if let Some(d) = &gpu_desc {
            crate::launch::log(&format!(
                "适配器：{d}{}",
                if gpu_software {
                    "（软件渲染）"
                } else {
                    ""
                }
            ));
        }

        // 清掉上次遗留的更新暂存目录 —— 留在盘上的旧安装包本身就是个可被替换的靶子
        crate::updater::cleanup_stale();

        let mut tools = views::registry();
        let builtin_tools = tools.len();
        // WASM 插件：内置工具之后追加（加载失败只提示，不影响启动）
        let (plugin_tools, plugin_warns) = crate::plugin_host::load_all();
        for t in plugin_tools {
            tools.push(Box::new(t));
        }
        for t in tools.iter_mut() {
            let id = t.meta().id;
            if let Some(data) = persist.drafts.get(id) {
                t.load_draft(data);
            }
        }
        let active = tools
            .iter()
            .position(|t| t.meta().id == persist.active_id)
            .unwrap_or(0);

        // 界面缩放在**第一帧之前**就落定。放到 update() 里意味着首帧按 100% 排完版、
        // 窗口显示出来、下一帧才跳到用户设定的比例 —— 那一跳就是开窗时的闪。
        let ui_scale = persist.ui_scale.clamp(0.8, 1.6);
        ctx.set_zoom_factor(ui_scale);

        let mut shared = Shared::new(theme);
        shared.lang = persist.lang;
        shared.code_font = persist.code_font.clamped();
        shared.gpu_software = gpu_software;
        // 软渲染环境的清晰度与流畅度自适应要在首帧前生效，否则第一屏就带着灰雾
        // 阴影、且第一帧就用最慢的羽化路径把整窗光栅化一遍。
        if gpu_software {
            Self::apply_soft_render_compat(ctx);
        }
        for w in plugin_warns {
            shared.toast(format!("插件加载失败 · {w}"));
        }
        // 没有中文字体 = 满屏方块。提示必须中英双语：这条消息本身也会是方块，
        // 英文那半句是用户唯一读得懂的部分。
        if !has_cjk {
            shared.toast(
                "未找到中文字体，界面会显示为方块 / No CJK font found: \
                 please install Microsoft YaHei or Noto Sans SC",
            );
        }

        Self {
            tools,
            active,
            mode,
            dark,
            rail_width: persist.rail_width.clamp(RAIL_MIN, RAIL_MAX),
            favorites: persist.favorites.into_iter().collect(),
            rail_filter: String::new(),
            focus_search: false,
            settings_open: false,
            server_override: persist.server,
            server_draft: None,
            fingerprint_input: String::new(),
            updater: crate::updater::Updater::default(),
            shared,
            shot_frames: 0,
            ui_scale,
            applied_ui_scale: Some(ui_scale),
            settings_raise: false,
            raise: SettingsRaise::default(),
            frames: 0,
            launch_cfg: crate::launch::load(),
            auto_update: persist.auto_update,
            // 新字段缺失（旧配置文件）就从旧的 mock_source 迁移过来
            source_pref: persist
                .source_pref
                .unwrap_or_else(|| crate::source::SourcePref::from_legacy(persist.mock_source)),
            github_override: persist
                .github_repo
                .map(|repo| crate::github::GithubSource { repo }),
            github_draft: None,
            last_update_check: persist.last_update_check,
            builtin_tools,
            was_focused: true,
            last_outer_pos: None,
            last_moved_at: None,
            fast_present: false,
            last_frame_at: None,
            settings_tab: SettingsTab::default(),
            gpu_desc,
            gpu_software,
            slow_render_retry: None,
            pending_restart: None,
            want_restart: false,
            update_dialog_open: false,
            soft_storage: None,
            mem_recorder: None,
        }
    }

    /// 当前生效的数据源（自建服务端 / GitHub 发布页 / 演示数据）。
    fn source(&self) -> Option<crate::source::Source> {
        crate::source::Source::resolve(
            self.source_pref,
            self.server_profile(),
            self.github_source(),
        )
    }

    /// 当前生效的 GitHub 源：用户设的优先，否则用编译期烘入的。
    fn github_source(&self) -> Option<crate::github::GithubSource> {
        self.github_override
            .clone()
            .or_else(crate::github::GithubSource::builtin)
    }

    /// 距上次成功检查更新是否已经够久（跨启动节流）。
    fn update_check_is_stale(&self) -> bool {
        match self.last_update_check {
            None => true,
            Some(t) => {
                now_unix().saturating_sub(t) >= crate::updater::AUTO_CHECK_INTERVAL_SECS as i64
            }
        }
    }

    /// 热加载插件：把插件那截工具换成磁盘上的当前状态，内置工具原封不动。
    ///
    /// 只能由**外壳**在一帧渲染结束之后调用 —— 视图自己正被 `self.tools[i]` 借着。
    ///
    /// 保留：当前选中的工具（按 id 找回，插件被卸载则回落到市场页）、
    /// 各插件的输入草稿（按 id 搬回去）。
    ///
    /// ⚠️ `PluginTool` 的 `ToolMeta` 需要 `&'static str`，加载时用 `Box::leak` 得到 ——
    /// 因此每次热加载都会漏掉一份插件元数据（每个插件几百字节）。装插件是低频动作，
    /// 这点代价换「不用重启」是划算的；但别把这个函数接到什么每帧调用的地方去。
    fn reload_plugins(&mut self) {
        let active_id = self.tools[self.active].meta().id.to_owned();
        // 插件的输入草稿：重建前先收起来，重建后按 id 放回去
        let drafts: HashMap<String, String> = self.tools[self.builtin_tools..]
            .iter()
            .filter_map(|t| t.save_draft().map(|d| (t.meta().id.to_owned(), d)))
            .collect();

        self.tools.truncate(self.builtin_tools);
        let (plugin_tools, warns) = crate::plugin_host::load_all();
        let loaded = plugin_tools.len();
        for mut t in plugin_tools {
            if let Some(d) = drafts.get(t.meta().id) {
                t.load_draft(d);
            }
            self.tools.push(Box::new(t));
        }
        for w in warns {
            self.shared.toast(format!("插件加载失败 · {w}"));
        }

        // 选中的工具可能刚被卸载 —— 找不回来就退回插件市场（用户就是从那儿来的）
        self.active = self
            .tools
            .iter()
            .position(|t| t.meta().id == active_id)
            .or_else(|| self.tools.iter().position(|t| t.meta().id == "market"))
            .unwrap_or(0);
        self.shared
            .toast(format!("插件已重新加载（当前 {loaded} 个）"));
    }

    /// 当前生效的更新服务器。None = 本构建未配置（更新功能整体禁用，
    /// **绝不回落到「去问服务端要公钥」**，那等于让对方自报家门）。
    fn server_profile(&self) -> Option<crate::net::ServerProfile> {
        self.server_override
            .clone()
            .or_else(crate::net::ServerProfile::builtin)
    }

    // 「是不是内置服务器」现在问数据源本身（`Source::is_builtin`）——
    // 演示数据也要参与这个判断，而它根本没有 ServerProfile。

    fn set_mode(&mut self, ctx: &egui::Context, mode: ThemeMode) {
        self.mode = mode;
        self.sync_theme(ctx);
    }

    /// 按当前模式解析生效深浅色；System 模式下读系统主题，变化时重建配色。
    /// 每帧调用（无变化时零开销），系统切换深浅色可实时跟随。
    fn sync_theme(&mut self, ctx: &egui::Context) {
        let want = match self.mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => ctx
                .system_theme()
                .map_or(self.dark, |t| t == egui::Theme::Dark),
        };
        if want != self.dark || self.shared.theme.dark != want {
            self.dark = want;
            self.shared.theme = Theme::from_dark(want);
            self.shared.theme.apply(ctx);
            // 主题重铺会把阴影写回来 —— 软渲染环境要立刻再剥掉（羽化在 Options 里，
            // 重铺不会动它，但一并调用保持三项适配同进同出）。
            if self.gpu_software {
                Self::apply_soft_render_compat(ctx);
            }
            // 换主题会重铺样式，字号缩放要跟着重新落一次
            self.applied_ui_scale = None;
        }
        self.sync_ui_scale(ctx);
    }

    /// 软件光栅化（WARP / llvmpipe）下的清晰度与流畅度自适应。
    ///
    /// 阴影是大范围半透明渐变：硬件渲染下是「柔和」，软渲染 + 低分屏下叠在内容
    /// 边上就是一圈灰雾 —— 用户的原话是「一片糊」。这类环境里全部清零，
    /// 界面靠 1px 边框与底色分层；直角、文本的像素对齐 egui 默认已开。
    /// 硬件渲染路径完全不受影响（本函数只在 `gpu_software` 时调用）。
    ///
    /// 同时把 `animation_time` 清零。egui 的每一段动画（悬停高亮渐变、折叠展开、
    /// 浮层淡入）在播放期间都会**每帧请求重绘整窗** —— 而 egui 没有局部重绘，
    /// 一帧就是把整个窗口重新排版、三角化、光栅化一遍。有 GPU 时这是白送的，
    /// 靠 CPU 画的时候正好相反：动画本身画不满帧，却把 CPU 占满，于是每次
    /// 鼠标扫过侧栏都能感到一次顿挫。没有动画 = 状态切换是瞬时的，
    /// 观感上「干脆」，代价只是少了几十毫秒的渐变。
    ///
    /// 最后关掉**抗锯齿羽化**（feathering）—— 这是「无 GPU 加速也要默认能平稳跑」
    /// 的关键一环。egui 给每个矢量形状（面板、圆角、边框、分隔线、导航项高亮）的
    /// 边缘都补一圈约 1px 的半透明过渡三角形来做抗锯齿：三角形数量因此翻倍，还要
    /// 逐像素做 alpha 混合，而这两样恰恰是软件光栅化最慢的路径。关掉后矢量边缘
    /// 硬一档，但**文字完全不受影响**（字形走的是字体图集纹理采样，不是羽化），
    /// 换来的是滚动 / 输入 / 拖动时肉眼可见的流畅度 —— 与上面「去动画、去阴影」
    /// 是同一套「这种环境优先流畅、清晰胜过柔和」的取舍。
    ///
    /// feathering 存在 [`egui::Options`] 里而非 `Style`，主题重铺（[`Theme::apply`]）
    /// 不会碰它，严格说设一次就够；仍放在这里，是让软渲染的三项适配同进同出、
    /// 切深浅色重铺样式时也不漏项。
    /// 还要把 `tooltip_delay` 清零 —— 这一条是**滚动卡顿的真凶**，且原因不在本仓库。
    ///
    /// egui 的 `containers/tooltip.rs` 里有这么一段：滚动后的 `tooltip_delay`（默认
    /// 0.5s）内不弹提示，并且**每帧**按「还差多久」申请一次重绘：
    /// `request_repaint_after_secs(tooltip_delay - time_since_last_scroll)`。
    /// 差值趋近 0 时，申请的延迟就小于一个定时器 tick（Windows 约 15.6ms），
    /// 于是立刻醒 → 算出更小的值 → 再醒，**自旋到延迟走完为止**。
    /// 实测：光标停在带提示的控件上滚一下，帧率飙到 **1500+ fps**，
    /// 40 格滚轮吃掉 **3.4 个核**（4 核机器），关窗还要 10 秒才排空。
    ///
    /// 置 0 后该分支（`time_since_last_scroll < tooltip_delay`）永不成立，自旋消失；
    /// 代价只是滚动后提示不再压 0.5s 才出现。有 GPU 时这点浪费看不出来，
    /// 靠 CPU 画整窗的机器上它是压垮性的，所以只在 `gpu_software` 路径上关。
    fn apply_soft_render_compat(ctx: &egui::Context) {
        ctx.all_styles_mut(|s| {
            s.visuals.window_shadow = egui::epaint::Shadow::NONE;
            s.visuals.popup_shadow = egui::epaint::Shadow::NONE;
            s.animation_time = 0.0;
            s.interaction.tooltip_delay = 0.0;
        });
        ctx.options_mut(|o| o.tessellation_options.feathering = false);
    }

    /// 软渲染下给帧率封顶（约 30fps）。**只在 `gpu_software` 时生效。**
    ///
    /// 为什么需要：egui 只要输入事件队列非空就要求「立即重绘」
    /// （`InputState::wants_repaint_after` 里 `!events.is_empty() → Duration::ZERO`），
    /// 而滚轮一拨就是一连串事件。实测滚动期间应用跑到 **500–1500 fps** ——
    /// 可这台机器靠 CPU 光栅化，实际只显示得出约 25–30fps。**多出来的 20–60 倍帧
    /// 一帧也看不见**，却照样要排版、三角化、提交，把 CPU 和提交队列全占满：
    /// 手停下来之后画面还要追好几秒，关窗时 wgpu 等最后一次提交要 10s 以上直至超时。
    ///
    /// 这里的 sleep 是**有意的背压**：让事件循环慢下来，帧率降到显示得出的量级。
    /// 代价是单帧输入延迟最多 +30ms —— 远小于它消掉的那几秒追帧。
    /// 有 GPU 时整个函数是空操作，一帧也不会被压。
    ///
    /// 拖动窗口时**同样要封顶**（曾经试过豁免，见 [`Self::present_hygiene`]）：
    /// 实测豁免后拖动期间冲到 1193 fps，而屏幕只显示得出约 30fps ——
    /// 多出来的帧全堆在提交队列里，于是**显示出来的那一帧是很多帧以前渲染的**，
    /// 对应的是旧窗口位置。拖影正是这么来的，放开帧率只会让它更重。
    /// # 移动窗口时必须整个跳过
    ///
    /// 这里的节流是 `thread::sleep`，而它睡在 **UI 线程**上 —— 消息泵也停在那儿。
    /// 拖动窗口走的是系统的模态移动循环，跑在同一个线程上：每帧睡 30ms，
    /// 窗框就得等 30ms 才能挪一步，看着就是「跟不上手 / 拖影」。
    ///
    /// 实测（同样 90 步程序化移动，`SetWindowPos` 会同步等目标线程的消息泵）：
    /// 上限 30ms → 移动耗时 2829ms；上限 100ms → **8924ms**，整整慢 3 倍。
    /// 压得越狠，窗口移动本身越卡 —— 与省 CPU 的初衷正好相反。
    ///
    /// 跳过后并不会回到之前 1193fps 的洪水：那个洪水的来源是
    /// [`Self::present_hygiene`] 里「移动期间每帧 `request_repaint`」，已经去掉了。
    /// 现在移动期间的帧只由真实输入事件驱动，数量本来就不多。
    fn throttle_soft_render(&mut self) {
        if !self.gpu_software || self.fast_present {
            return;
        }
        const MIN_FRAME: std::time::Duration = std::time::Duration::from_millis(30);
        if let Some(prev) = self.last_frame_at {
            let dt = prev.elapsed();
            if dt < MIN_FRAME {
                std::thread::sleep(MIN_FRAME - dt);
            }
        }
        self.last_frame_at = Some(std::time::Instant::now());
    }

    /// 全局界面缩放。
    ///
    /// 走 `zoom_factor` 而不是缩放 `text_styles`：本应用的字号绝大多数是在各处
    /// 用 `RichText::size(..)` 写死的（近百处），改 text_styles 只能影响到少数
    /// 走默认样式的控件 —— 实测只有搜索框的提示文字会变。zoom_factor 作用在
    /// `pixels_per_point` 上，文字、间距、图标一起放大，才是用户要的「整体变大」。
    fn sync_ui_scale(&mut self, ctx: &egui::Context) {
        let scale = self.ui_scale.clamp(0.8, 1.6);
        if self.applied_ui_scale == Some(scale) {
            return; // 没变就不动，避免每帧触发一次重排
        }
        ctx.set_zoom_factor(scale);
        self.applied_ui_scale = Some(scale);
    }

    /// 按 group 分组（保序）返回 (组名, 工具索引列表)。
    fn grouped(&self) -> Vec<(&'static str, Vec<usize>)> {
        let mut order: Vec<&'static str> = Vec::new();
        let mut map: HashMap<&'static str, Vec<usize>> = Default::default();
        for (i, t) in self.tools.iter().enumerate() {
            let g = t.meta().group;
            if !order.contains(&g) {
                order.push(g);
            }
            map.entry(g).or_default().push(i);
        }
        order
            .into_iter()
            .map(|g| (g, map.remove(g).unwrap()))
            .collect()
    }

    // ---------- 侧栏 ----------

    fn rail_ui(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;

        // 搜索框
        ui.add_space(6.0);
        Frame::NONE
            .fill(theme.code_bg)
            .corner_radius(CornerRadius::same(11))
            .inner_margin(Margin::symmetric(12, 0))
            .show(ui, |ui| {
                ui.set_height(40.0);
                ui.horizontal_centered(|ui| {
                    ui.label(icons::text(icons::SEARCH, 16.0, theme.muted));
                    ui.add_space(8.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.rail_filter)
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY)
                            .hint_text(RichText::new("搜索工具…  Ctrl K").color(theme.faint)),
                    );
                    if self.focus_search {
                        resp.request_focus();
                        self.focus_search = false;
                    }
                });
            });
        ui.add_space(4.0);

        // 底部品牌 / 主题 / 关于 / 设置
        Panel::bottom("rail-foot")
            .frame(Frame::NONE.inner_margin(Margin {
                left: 2,
                right: 2,
                top: 6,
                bottom: 2,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().top(),
                    Stroke::new(1.0_f32, theme.border),
                );
                ui.add_space(7.0);
                // 品牌一行（logo + 名称 + 版本同排，见 `brand`）
                ui.horizontal(|ui| {
                    self.brand(ui);
                });
                ui.add_space(5.0);
                // 图标一行（主题 / 关于 / 设置 / 语言），紧凑左对齐
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let tmoon = if self.dark { icons::SUN } else { icons::MOON };
                    let resp = widgets::icon_btn(ui, &theme, tmoon, 18.0)
                        .on_hover_text("切换深浅色（设置中可改回跟随系统）");
                    if resp.clicked() {
                        // 快捷切换即视为手动锁定；想恢复跟随系统去设置里选。
                        let mode = if self.dark {
                            ThemeMode::Light
                        } else {
                            ThemeMode::Dark
                        };
                        self.set_mode(ui.ctx(), mode);
                    }
                    if widgets::icon_btn(ui, &theme, icons::INFO, 18.0).clicked() {
                        self.shared.toast(concat!(
                            "Ferric v",
                            env!("FERRIC_VERSION"),
                            " (build ",
                            env!("FERRIC_BUILD_NUMBER"),
                            ") · 本地开发者工具箱"
                        ));
                    }
                    if widgets::icon_btn(ui, &theme, icons::SETTINGS, 18.0).clicked() {
                        // 已经开着的时候再点，等于「把它翻到前面来」——设置窗是独立
                        // 系统窗口，会被别的窗口盖住，此时只置 true 是没有任何反馈的。
                        // 从没开过则不必置顶：窗口是新建的，本来就在最前。
                        self.settings_raise = self.settings_open;
                        self.settings_open = true;
                    }
                    // 语言切换（显示当前语言，点击切换 中/EN）。
                    // 尺寸与 `widgets::icon_btn(.., 18.0)` 算出来的方块保持一致，
                    // 否则这一排四个按钮会有一个明显更大。
                    let side = 18.0_f32 * 1.65;
                    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(9), theme.border);
                    }
                    let lcol = if resp.hovered() {
                        theme.fg
                    } else {
                        theme.muted
                    };
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        self.shared.lang.short(),
                        FontId::proportional(13.0),
                        lcol,
                    );
                    if resp.on_hover_text("切换语言 / Language").clicked() {
                        self.shared.lang = self.shared.lang.toggled();
                    }
                });
            });

        // 导航列表
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);
                let filter = self.rail_filter.to_lowercase();
                // 收藏分组：置顶展示（点顶栏 ❤ 收藏的工具），搜索时同样过滤。
                let favs: Vec<usize> = (0..self.tools.len())
                    .filter(|i| self.favorites.contains(self.tools[*i].meta().id))
                    .filter(|i| self.matches(*i, &filter))
                    .collect();
                if !favs.is_empty() {
                    self.group_label(ui, "收藏");
                    for i in favs {
                        self.nav_item(ui, i);
                    }
                    ui.add_space(10.0);
                }
                for (group, indices) in self.grouped() {
                    let visible: Vec<usize> = indices
                        .into_iter()
                        .filter(|i| self.matches(*i, &filter))
                        .collect();
                    if visible.is_empty() {
                        continue;
                    }
                    self.group_label(ui, group);
                    for i in visible {
                        self.nav_item(ui, i);
                    }
                    ui.add_space(10.0);
                }
            });
    }

    fn matches(&self, idx: usize, filter: &str) -> bool {
        if filter.is_empty() {
            return true;
        }
        let m = self.tools[idx].meta();
        m.name.to_lowercase().contains(filter) || m.keywords.iter().any(|k| k.contains(filter))
    }

    /// 侧栏底部的品牌行：logo + 名称 + 版本，**排成一行**。
    ///
    /// 原先是 34px 的大方块配上「Ferric / v0.2.14 · rust」两行堆叠，连着下面那排
    /// 图标按钮，侧栏底部占掉一百多像素 —— 视觉重量全压在左下角，而这块信息
    /// 用户一天看不了一次。压成一行后底部从 ~106px 收到 ~72px。
    ///
    /// 版本后面那个「· rust」也去掉了：用什么语言写的是作者的自我介绍，不是用户
    /// 需要的信息；真要查版本，ℹ 按钮给的是带 build 号的完整串。
    fn brand(&self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        // 渐变方块 logo（用 accent 填色近似渐变）
        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(7), theme.accent);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icons::BOX,
            FontId::new(13.0, icons::family()),
            Color32::WHITE,
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new("Ferric")
                .family(FontFamily::Name(UI_BOLD.into()))
                .size(13.0)
                .color(theme.fg),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(concat!("v", env!("FERRIC_VERSION")))
                .family(FontFamily::Monospace)
                .size(10.5)
                .color(theme.faint),
        );
    }

    fn group_label(&self, ui: &mut egui::Ui, group: &str) {
        let theme = self.shared.theme;
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(icons::text(group_icon(group), 12.0, theme.faint));
            ui.add_space(6.0);
            ui.label(
                RichText::new(group)
                    .family(FontFamily::Name(UI_SEMIBOLD.into()))
                    .size(11.0)
                    .color(theme.faint),
            );
        });
        ui.add_space(2.0);
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, idx: usize) {
        let theme = self.shared.theme;
        let meta = self.tools[idx].meta();
        let selected = idx == self.active;

        let h = 36.0;
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let hovered = resp.hovered();

        let fill = if selected {
            theme.accent_soft
        } else if hovered {
            theme.border
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
        }
        let icon_col = if selected { theme.accent } else { theme.muted };
        let text_col = if selected {
            theme.accent_strong
        } else {
            theme.fg_soft
        };
        // 图标
        ui.painter().text(
            rect.left_center() + vec2(12.0, 0.0),
            Align2::LEFT_CENTER,
            meta.icon,
            FontId::new(18.0, icons::family()),
            icon_col,
        );
        // 名称
        let name_family = if selected {
            FontFamily::Name(UI_SEMIBOLD.into())
        } else {
            FontFamily::Proportional
        };
        ui.painter().text(
            rect.left_center() + vec2(41.0, 0.0),
            Align2::LEFT_CENTER,
            meta.name,
            FontId::new(13.5, name_family),
            text_col,
        );
        ui.add_space(2.0);
        if resp.clicked() {
            self.active = idx;
            // 切换工具后立即请求重绘：与 present_hygiene 的「焦点上升沿立即重绘」
            // 同理 —— DX12 flip-model 下交换链里还留着上一帧，切换瞬间旧内容多
            // 停留一帧就是「切一下闪一下」。显式请求把新内容压到下一帧就上屏。
            ui.ctx().request_repaint();
        }
    }

    // ---------- 内容区 ----------

    /// 「当前只有软件渲染，已为下次启动排好别的后端」横幅。
    ///
    /// 为什么是常驻横幅而不是 toast：这条消息**要求用户做一件事**（重启），
    /// 而 toast 三秒就没了 —— 卡的时候用户正忙着跟界面较劲，很可能压根没看见。
    /// 也不放进设置窗：能想到去翻设置的人本来就不需要提示。
    ///
    /// 只在「还有没试过的后端」时出现。全都试过仍是软件渲染的机器上不显示 ——
    /// 那种情况重启多少次都一样，横幅就成了赶不走的噪音（原因写在设置页里）。
    fn slow_render_banner(&mut self, ui: &mut egui::Ui) {
        let Some(next) = self.slow_render_retry else {
            return;
        };
        let theme = self.shared.theme;
        Panel::top("slow-render-banner")
            .exact_size(38.0)
            // 纯色底 + 无阴影：这条横幅恰恰只在软件渲染时出现，
            // 而半透明/阴影正是那种环境下「一片糊」的来源。
            .frame(
                Frame::NONE
                    .fill(theme.code_bg)
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(icons::text(icons::INFO, 13.0, theme.danger));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "当前没有 GPU 加速（{}），界面会明显发卡 · 已为下次启动排好「{}」",
                            self.gpu_desc.as_deref().unwrap_or("软件渲染"),
                            next.label()
                        ))
                        .size(12.0)
                        .color(theme.danger),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::subtle_button(ui, &theme, None, "以后再说").clicked() {
                            self.slow_render_retry = None;
                        }
                        ui.add_space(6.0);
                        if widgets::primary_button(ui, &theme, "立即重启").clicked() {
                            self.restart_now();
                        }
                    });
                });
            });
    }

    /// 请求重启。**只置位**，真正的动作在 [`FerricApp::do_restart`] ——
    /// 那里才拿得到 `eframe::Frame`，而重启前必须先把草稿落盘（理由见彼处）。
    /// 与插件热加载同一套「置位、由外壳在帧末统一处理」的写法。
    fn restart_now(&mut self) {
        self.want_restart = true;
    }

    /// 把当前应用状态序列化成可持久化的 [`Persist`]。`eframe::App::save` 与
    /// [`FerricApp::save_soft`] 共用，避免两条渲染路径各写一份。
    fn persist(&self) -> Persist {
        let drafts = self
            .tools
            .iter()
            .filter_map(|t| t.save_draft().map(|d| (t.meta().id.to_owned(), d)))
            .collect();
        Persist {
            dark: self.dark,
            theme_mode: Some(self.mode),
            rail_width: self.rail_width,
            favorites: self.favorites.iter().cloned().collect(),
            active_id: self.tools[self.active].meta().id.to_owned(),
            drafts,
            lang: self.shared.lang,
            server: self.server_override.clone(),
            ui_scale: self.ui_scale,
            code_font: self.shared.code_font,
            auto_update: self.auto_update,
            // 两个字段一起写：新版本读 source_pref，旧版本读 mock_source。
            // 只写新的，用户装回旧版本时设置就没了
            mock_source: self.source_pref.to_legacy(),
            source_pref: Some(self.source_pref),
            github_repo: self.github_override.as_ref().map(|g| g.repo.clone()),
            last_update_check: self.last_update_check,
        }
    }

    /// 软渲染路径的落盘：把当前状态写进 [`FerricApp::new_soft`] 注入的 storage 并 flush。
    /// eframe 路径下 storage 为 `None`，这里是空操作（走 `eframe::App::save`）。
    pub fn save_soft(&mut self) {
        // 先不可变借出持久化快照，再可变借用 storage，避免两个借用交叠。
        let persist = self.persist();
        if let Some(storage) = self.soft_storage.as_deref_mut() {
            eframe::set_value(storage, eframe::APP_KEY, &persist);
            storage.flush();
        }
    }

    /// 真正执行重启：先存盘，再拉起新进程，最后关掉自己。
    ///
    /// **顺序很要紧**。新进程一起来就会去读 eframe 的状态文件；而本次会话改过的
    /// 草稿（JSON 正文、各工具的输入）要等关窗走 `App::save` 才写下去 ——
    /// 先 spawn 后关窗的话，两个进程会赛跑，新实例大概率读到上一次的旧草稿，
    /// 表现就是「点了重启，我刚才编辑的内容没了」。所以这里显式先存一次并 flush。
    ///
    /// 失败就说清楚，绝不静默 —— 「点了没反应」比没有这个按钮更糟。
    fn do_restart(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(storage) = frame.storage_mut() {
            eframe::App::save(self, storage);
            storage.flush();
        } else {
            // 软渲染路径没有 eframe::Frame 的 storage，用注入的 storage 落盘。
            self.save_soft();
        }
        match crate::launch::relaunch() {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => {
                self.slow_render_retry = None;
                self.shared
                    .toast(format!("自动重启失败，请手动重开 Ferric · {e}"));
            }
        }
    }

    fn topbar_ui(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        let meta = self.tools[self.active].meta();
        let id = meta.id;
        let is_fav = self.favorites.contains(id);
        let (mut side, _) = content_metrics(ui.available_width());
        if self.tools[self.active].full_bleed() {
            side = 24.0; // 铺满模式：标题贴左，不随居中列缩进
        }
        ui.horizontal_centered(|ui| {
            ui.add_space(side);
            ui.label(
                RichText::new(meta.name)
                    .family(FontFamily::Name(UI_BOLD.into()))
                    .size(18.0)
                    .color(theme.fg),
            );
            ui.add_space(14.0);
            // 工具专属操作（如 JSON 工具条）紧随标题
            self.tools[self.active].header_actions(ui, &mut self.shared);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(side);
                // 更新入口就放在这儿：更新是全局的事，挂在设置窗里意味着
                // 「东西早就下好了，但用户永远不知道」。
                self.update_badge(ui);
                let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
                if resp.hovered() {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(9), theme.border);
                }
                let hcol = if is_fav {
                    theme.accent
                } else if resp.hovered() {
                    theme.fg
                } else {
                    theme.muted
                };
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    icons::HEART,
                    FontId::new(18.0, icons::family()),
                    hcol,
                );
                if resp.clicked() {
                    if self.favorites.contains(id) {
                        self.favorites.remove(id);
                    } else {
                        self.favorites.insert(id.to_owned());
                    }
                }
            });
        });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.max_rect().bottom(),
            Stroke::new(1.0_f32, theme.border),
        );
    }

    /// 顶栏右侧的更新入口：下载中显示进度，就绪时是一个「安装 vX」按钮。
    ///
    /// 点它 = 拉起安装程序并退出本进程。**只有内置服务器的包才允许这么做**
    /// （演示数据与自定义源都只提示，见 `Source::allows_install`）——
    /// 这一步是把一个下载来的文件交给操作系统执行，门槛必须是最高的那一档。
    fn update_badge(&mut self, ui: &mut egui::Ui) {
        use crate::updater::Phase;
        let theme = self.shared.theme;
        match self.updater.phase.clone() {
            Phase::Downloading { done, total } => {
                // 与设置页那处同一种写法：`if total > 0 { .. / total }` 会被
                // clippy 的 manual_checked_ops 拦下（CI 是 -D warnings）
                let pct = done
                    .checked_mul(100)
                    .and_then(|x| x.checked_div(total))
                    .unwrap_or(0);
                ui.label(
                    RichText::new(format!("更新下载中 {pct}%"))
                        .size(11.5)
                        .color(theme.muted),
                );
            }
            Phase::Ready { info, file } => {
                let can_install = self.source().is_some_and(|s| s.allows_install());
                if widgets::primary_button(ui, &theme, &format!("安装 v{}", info.version))
                    .on_hover_text(if can_install {
                        "已下载并通过 sha256 与签名校验 · 点击安装（将退出 Ferric）"
                    } else {
                        "演示数据 / 自定义更新源不会真的安装"
                    })
                    .clicked()
                {
                    if can_install {
                        match crate::updater::launch(&file) {
                            Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => {
                                self.updater.phase = Phase::Failed(e);
                            }
                        }
                    } else {
                        self.shared
                            .toast("演示模式：不会真的执行安装程序（换成真实更新源即可）");
                    }
                }
            }
            _ => {}
        }
    }

    fn content_body(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        let i = self.active;
        let meta = self.tools[i].meta();

        // 铺满模式：整个内容区（宽 100% × 高 100%）直接交给工具，
        // 工具内部用面板自行划分（如 JSON：底部状态条 + 其余全是编辑区）。
        if self.tools[i].full_bleed() {
            self.tools[i].ui(ui, &mut self.shared);
            return;
        }

        let (side, colw) = content_metrics(ui.available_width());

        // page-intro：4px accent 竖条 + 描述（工具可选择不显示）
        if self.tools[i].show_desc() {
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add_space(side);
                let (bar, _) = ui.allocate_exact_size(vec2(4.0, 34.0), Sense::hover());
                ui.painter()
                    .rect_filled(bar, CornerRadius::same(3), theme.accent);
                ui.add_space(12.0);
                ui.add(
                    egui::Label::new(RichText::new(meta.desc).size(14.0).color(theme.muted)).wrap(),
                );
            });
            ui.add_space(14.0);
        } else {
            ui.add_space(10.0);
        }

        // 进入滚动区前记录真实可用高度（滚动区内 available_height 不可靠）。
        self.shared.content_height = ui.available_height();

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_width(colw);
                        self.tools[i].ui(ui, &mut self.shared);
                    });
                });
                ui.add_space(4.0);
            });
    }

    // ---------- 窗口边框 ----------

    fn window_border_ui(&self, ctx: &egui::Context) {
        draw_window_border(ctx, self.shared.theme.dark, "window-border");
    }
}

/// 沿窗口边缘画**一条 1px 实线边框**。
///
/// 无边框方角窗口下与轮廓完全贴合，用来和桌面上别的窗口区分开。主窗与设置窗
/// **共用这一份**：设置窗底色与主界面一样，没有这圈边框两者叠在一起就分不出边界。
/// `id` 要各用各的 —— 同一个图层 id 会让两个视口互相覆盖。
///
/// 此前是三圈半透明「内发光」渐变 —— 硬件渲染下尚且低调，软件光栅化（虚拟机）
/// 里多层 alpha 合成直接糊成一圈脏边。现在只画一条不透明细线：
/// shrink(0.5) 让 1px 描边落在半像素网格上，任何渲染路径出来都是锐利的一条。
fn draw_window_border(ctx: &egui::Context, dark: bool, id: &str) {
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new(id),
    ));
    // 亮色主题用深灰，暗色主题提亮一档保证可见。不透明 —— 反差不靠猜。
    let c = if dark {
        Color32::from_rgb(150, 150, 155)
    } else {
        Color32::from_rgb(60, 60, 66)
    };
    painter.rect_stroke(
        rect.shrink(0.5),
        CornerRadius::ZERO,
        Stroke::new(1.0_f32, c),
        egui::StrokeKind::Middle,
    );
}

impl FerricApp {
    // ---------- 设置弹窗 ----------

    /// 设置窗：一个**独立的系统窗口**，与主界面同级。
    ///
    /// 用 `show_viewport_immediate` 开真实 OS 窗口，而不是应用内的 `egui::Window` ——
    /// 后者只能在主窗范围内挪动，拖不出去；独立窗口由窗口管理器接管，可以拖到屏幕
    /// 任何位置、也能和主界面并排摆。
    ///
    /// 选 immediate 而非 deferred：immediate 在同一帧里执行闭包，可以直接借用
    /// `&mut self`；deferred 要求闭包 'static，得把整份状态塞进 Arc<Mutex> 才行。
    ///
    /// 外壳沿用主窗那套：无边框 + 自绘标题栏 + `StartDrag`，这样与主界面观感一致，
    /// 拖动同样交给系统（因而能拖满整个屏幕）。
    fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            self.raise = SettingsRaise::default();
            return;
        }
        // 设置窗**一律**开真实系统窗口 —— 与主界面同一层级，能拖到屏幕任何位置。
        //
        // 这里曾经按 `gpu_software` 一刀切回退成应用内浮层，理由是「软件适配器
        // 建第二条 wgpu surface 常常失败或渲染空白」。**这个判断过于宽泛**：
        // 在 ARM 虚拟机 + WARP（DX12 · Microsoft Basic Render Driver，货真价实的
        // 软件光栅化）上实测，第二个视口建得出来、内容完整渲染、进程稳定 ——
        // 却因为这条规则永远拿不到独立窗口。「软件渲染」不等于「开不了第二条 surface」，
        // 前者按适配器类别判定，后者取决于具体驱动，只有试过才知道。
        //
        // 真遇到建不出来的环境（历史上的嫌疑是 Linux + llvmpipe），
        // 用 `FERRIC_EMBEDDED_SETTINGS=1` 走应用内浮层回退：同一份 settings_body，
        // 功能不缺项，只是拖不出主窗。回退路径保留、随时可用，只是不再自动生效。
        if std::env::var_os("FERRIC_EMBEDDED_SETTINGS").is_some() {
            self.settings_embedded_ui(ctx);
            return;
        }
        let theme = self.shared.theme;
        let mut open = true;
        let mut focused = false;

        // 置顶请求交给状态机处理（细节见 `SettingsRaise`）。
        let raise = std::mem::take(&mut self.settings_raise);
        if raise {
            self.raise.request();
        }
        if self.raise.busy() {
            ctx.request_repaint(); // 处理期间保持出帧，否则计数推不下去
        }
        if self.raise.tick(recreate_is_the_only_raise()) {
            // 这一帧不 show 这个视口 → eframe 会销毁对应的系统窗口，下一帧重新建出来
            return;
        }

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("ferric-settings"),
            egui::ViewportBuilder::default()
                .with_title("Ferric 设置")
                // 左右布局要同时放下「分类栏 + 标签 + 控件」，428 宽时右栏只剩不到
                // 300px，四档 seg 会挤到换行。
                .with_inner_size([760.0, 560.0])
                .with_min_inner_size([620.0, 360.0])
                .with_decorations(false)
                .with_resizable(true),
            |ui, _class| {
                let ctx = ui.ctx().clone();
                // 关窗按钮 / 系统关闭请求
                if ctx.input(|i| i.viewport().close_requested()) {
                    open = false;
                }
                // 置顶第一招：`Focus`。必须在**设置窗自己的视口上下文里**发，
                // 发到主窗上只会把主窗提前。生效与否由外面那段等待逻辑判定。
                if raise {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
                settings_window_chrome(ui, &theme, &mut open);
                // 与主界面同款的 1px 边框：两窗底色一致，没有它就看不出边界在哪
                draw_window_border(&ctx, theme.dark, "settings-border");
                CentralPanel::default()
                    .frame(Frame::NONE.fill(theme.bg).inner_margin(Margin::same(18)))
                    // 不在这里套 ScrollArea：滚动归右栏自己管，分类栏要始终可见。
                    .show(ui, |ui| {
                        self.settings_body(ui);
                    });
            },
        );
        self.settings_open = open;
        self.raise.focused = focused;
    }

    /// 设置窗的内容（与承载它的窗口形态无关）。
    ///
    /// **左分类 + 右内容**，与主界面「侧栏 + 内容区」同一套结构与观感
    /// （见 [`SettingsTab`] 说明为什么不再是一长列）。
    ///
    /// 纵向滚动放在**右栏内部**：分类栏必须始终可见，否则滚到底就找不到路回去了。
    /// 承载它的两种窗口（独立视口 / 应用内浮层）因此都不再自己套 `ScrollArea`。
    fn settings_body(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        const NAV_W: f32 = 132.0;

        ui.horizontal_top(|ui| {
            // ---------- 左：分类 ----------
            ui.allocate_ui_with_layout(
                vec2(NAV_W, ui.available_height()),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(NAV_W);
                    for (tab, icon, name) in SettingsTab::ALL {
                        self.settings_nav_row(ui, tab, icon, name);
                    }
                },
            );

            // 竖分隔线：与主界面侧栏/内容区之间那条同款，1px 不透明
            ui.add_space(10.0);
            let sep = ui.available_rect_before_wrap();
            ui.painter().vline(
                sep.left(),
                sep.y_range(),
                Stroke::new(1.0_f32, theme.border),
            );
            ui.add_space(16.0);

            // ---------- 右：内容 ----------
            ui.vertical(|ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.settings_tab {
                        SettingsTab::Appearance => self.settings_appearance_ui(ui),
                        SettingsTab::Font => self.font_settings_ui(ui),
                        SettingsTab::Renderer => self.renderer_settings_ui(ui),
                        SettingsTab::Update => self.update_settings_ui(ui),
                        SettingsTab::Data => self.settings_data_ui(ui),
                        SettingsTab::About => self.settings_about_ui(ui),
                    });
            });
        });
    }

    /// 分类栏的一行。刻意与主侧栏的 [`Self::nav_item`] 用同一套视觉
    /// （选中填 `accent_soft`、悬停填 `border`、图标 + 文字左对齐），
    /// 这样两处导航看着是同一个东西，而不是「设置里另有一套控件」。
    fn settings_nav_row(&mut self, ui: &mut egui::Ui, tab: SettingsTab, icon: char, name: &str) {
        let theme = self.shared.theme;
        let selected = self.settings_tab == tab;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
        let fill = if selected {
            theme.accent_soft
        } else if resp.hovered() {
            theme.border
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
        }
        ui.painter().text(
            rect.left_center() + vec2(11.0, 0.0),
            Align2::LEFT_CENTER,
            icon,
            FontId::new(16.0, icons::family()),
            if selected { theme.accent } else { theme.muted },
        );
        ui.painter().text(
            rect.left_center() + vec2(34.0, 0.0),
            Align2::LEFT_CENTER,
            name,
            FontId::new(
                13.0,
                if selected {
                    FontFamily::Name(UI_SEMIBOLD.into())
                } else {
                    FontFamily::Proportional
                },
            ),
            if selected {
                theme.accent_strong
            } else {
                theme.fg_soft
            },
        );
        ui.add_space(2.0);
        if resp.clicked() {
            self.settings_tab = tab;
        }
    }

    /// 「外观」分类：主题、界面缩放、侧边栏宽度 —— 都是影响整个外壳长相的项。
    fn settings_appearance_ui(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "主题");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let sel = match self.mode {
                    ThemeMode::System => 0,
                    ThemeMode::Light => 1,
                    ThemeMode::Dark => 2,
                };
                if let Some(n) = widgets::seg(ui, &theme, &["跟随系统", "亮色", "暗色"], sel)
                {
                    let want = match n {
                        1 => ThemeMode::Light,
                        2 => ThemeMode::Dark,
                        _ => ThemeMode::System,
                    };
                    if want != self.mode {
                        self.set_mode(ui.ctx(), want);
                    }
                }
            });
        });
        ui.separator();
        self.ui_scale_settings_ui(ui);
        ui.separator();
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "侧边栏宽度");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::ghost_button(ui, &theme, "恢复默认").clicked() {
                    self.rail_width = RAIL_DEFAULT;
                }
            });
        });
    }

    /// 「数据」分类：本机存了什么、怎么清掉。
    fn settings_data_ui(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "本地数据");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::ghost_button(ui, &theme, "清除收藏与工具草稿").clicked() {
                    self.favorites.clear();
                    // 草稿在 save() 时由工具状态重建，重置工具即清除草稿。
                    self.tools = views::registry();
                    self.shared.toast("已清除收藏与全部工具草稿");
                }
            });
        });
    }

    /// 「关于」分类：版本与隐私说明。
    fn settings_about_ui(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "版本");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(concat!("v", env!("FERRIC_VERSION")))
                        .family(FontFamily::Monospace)
                        .size(12.0)
                        .color(theme.fg),
                );
            });
        });
        ui.separator();
        ui.label(
            // 接入自动更新后，「不上传」不再成立：检查更新会把本机版本号
            // 发给更新服务器。文案要如实，不能留一句已经不真的宣称。
            RichText::new("工具数据仅存于本机；仅检查更新时联网")
                .size(11.5)
                .color(theme.faint),
        );
        ui.add_space(12.0);
        // 数据目录按钮：`launch.json` 与 `startup.log` 都在这里。
        // 诊断报告（见 [`startup_diag`]）需要用户能方便地把这两份交出来。
        ui.horizontal(|ui| {
            if widgets::ghost_button(ui, &theme, "打开数据文件夹").clicked() {
                if let Err(e) = crate::launch::open_data_dir() {
                    self.shared.toast(format!("打开失败：{e}"));
                }
            }
            // 直接显示路径，省得用户点开再翻 —— 大多数"我这日志在哪"的问题
            // 一眼就能答（特别是 ARM Windows 上 `eframe::storage_dir` 走
            // `%LOCALAPPDATA%` 但有版本差异，肉眼看一眼比记忆稳）。
            if let Some(p) = crate::launch::data_dir() {
                ui.label(
                    RichText::new(p.display().to_string())
                        .family(FontFamily::Monospace)
                        .size(11.0)
                        .color(theme.faint),
                );
            }
        });
        // 内存采样：按需触发 30 秒录制，写到 `memory.log`（同数据目录）。
        // 默认不工作——点按钮才采，采完落盘 + toast。详见 `crate::mem`。
        ui.horizontal(|ui| {
            match self.mem_recorder.as_ref() {
                None => {
                    if widgets::ghost_button(ui, &theme, "记录 30 秒内存").clicked() {
                        self.start_mem_recording();
                    }
                }
                Some(rec) => {
                    // 录制中：按钮变占位文本，进度提示在右。
                    ui.add_enabled_ui(false, |ui| {
                        let _ = widgets::ghost_button(ui, &theme, "记录 30 秒内存");
                    });
                    ui.label(
                        RichText::new(format!(
                            "正在记录… {} / {} 秒",
                            rec.elapsed_secs(),
                            rec.duration_secs()
                        ))
                        .size(11.5)
                        .color(theme.muted),
                    );
                }
            }
        });
     }

    /// 设置窗的**应用内回退形态**：软件渲染环境专用（见 [`Self::settings_ui`]）。
    ///
    /// 用 `egui::Window` 而不是第二个系统视口：不新建 wgpu surface，主窗能画
    /// 出来它就一定能画出来 —— 这条路径的存在意义就是「无论环境多受限都打得开」。
    fn settings_embedded_ui(&mut self, ctx: &egui::Context) {
        // 独立窗口那套置顶状态机在浮层形态下没有意义，请求直接吞掉。
        let _ = std::mem::take(&mut self.settings_raise);
        self.raise = SettingsRaise::default();

        let theme = self.shared.theme;
        let mut keep_open = true;
        egui::Window::new(
            RichText::new("设置")
                .size(14.0)
                .color(theme.fg_soft)
                .family(FontFamily::Name(crate::fonts::UI_SEMIBOLD.into())),
        )
        // id 带布局版本号：`egui::Window` 会把用户调过的尺寸记在 egui memory 里，
        // 改了 default_size 也顶不过那份旧记录（老用户会拿到 432 宽的窄浮层，
        // 左右布局直接被挤坏）。换 id = 丢掉旧几何、按新默认值重新摆一次。
        .id(egui::Id::new("ferric-settings-embedded-v2"))
        .open(&mut keep_open)
        .collapsible(false)
        .resizable(true)
        // 与独立视口那条路同宽：同一份左右布局，不该因为承载形态不同而挤成两副样子。
        // 主窗放不下就按主窗收窄（浮层不能比它的宿主还宽）。
        .default_size([
            760.0_f32.min(ctx.content_rect().width() - 48.0).max(560.0),
            560.0,
        ])
        .default_pos(ctx.content_rect().center() - egui::vec2(380.0, 280.0))
        .min_width(520.0)
        .frame(
            Frame::window(&ctx.global_style())
                .fill(theme.bg)
                .inner_margin(Margin::same(18)),
        )
        .show(ctx, |ui| {
            // 高度封顶到主窗内，不让浮层顶出屏幕。滚动由右栏内部负责（见 settings_body）。
            let max_h = (ctx.content_rect().height() - 140.0).max(240.0);
            ui.set_max_height(max_h);
            self.settings_body(ui);
        });
        if !keep_open {
            self.settings_open = false;
        }
    }

    /// 「外观」里的界面缩放。
    ///
    /// 与代码字号刻意分开、也分在不同分类里：界面缩放影响整个外壳（侧栏、按钮、
    /// 说明文字），而 JSON 这类编辑区常常要单独调大来核对密钥、或单独调小来纵览
    /// 长文档，把它们绑在一起反而难用。
    fn ui_scale_settings_ui(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "界面缩放");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // 这一行在 right_to_left 布局里，seg 会倒着排。所以按**自然顺序**
                // 定义档位，绘制前反转、拿到下标再反转回来 —— 免得代码里写成
                // 「特大 大 标准 小」那种别扭样子。
                const STEPS: [(f32, &str); 4] =
                    [(0.9, "90%"), (1.0, "100%"), (1.15, "115%"), (1.3, "130%")];
                let names: Vec<&str> = STEPS.iter().rev().map(|(_, n)| *n).collect();
                let nat = STEPS
                    .iter()
                    .position(|(v, _)| (*v - self.ui_scale).abs() < 0.01)
                    .unwrap_or(1);
                if let Some(n) = widgets::seg(ui, &theme, &names, STEPS.len() - 1 - nat) {
                    self.ui_scale = STEPS[STEPS.len() - 1 - n].0;
                }
            });
        });
    }

    /// 「字体」分类：代码编辑区的排版。
    ///
    /// 这三项与 JSON 工具条上的字体菜单是**同一份配置**，从哪边改都一样生效。
    fn font_settings_ui(&mut self, ui: &mut egui::Ui) {
        use crate::widgets::FontCfg;
        let theme = self.shared.theme;

        // —— 代码字号
        let font = &mut self.shared.code_font;
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "代码字号");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::ghost_button(ui, &theme, "+").clicked() {
                    font.size += 1.0;
                }
                ui.label(
                    RichText::new(format!("{} px", font.size as i32))
                        .family(FontFamily::Monospace)
                        .size(12.0)
                        .color(theme.fg),
                );
                if widgets::ghost_button(ui, &theme, "−").clicked() {
                    font.size -= 1.0;
                }
            });
        });

        // —— 字重 / 行距
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "代码字重");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // 同样按自然顺序（常规 → 中黑）呈现
                if let Some(n) =
                    widgets::seg(ui, &theme, &["中黑", "常规"], usize::from(!font.medium))
                {
                    font.medium = n == 0;
                }
            });
        });
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "代码行距");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let last = FontCfg::LINE_SCALES.len() - 1;
                let names: Vec<&str> = FontCfg::LINE_SCALES.iter().rev().map(|(_, n)| *n).collect();
                let nat = FontCfg::LINE_SCALES
                    .iter()
                    .position(|(v, _)| (*v - font.line_scale).abs() < 0.01)
                    .unwrap_or(1);
                if let Some(n) = widgets::seg(ui, &theme, &names, last - nat) {
                    font.line_scale = FontCfg::LINE_SCALES[last - n].0;
                }
            });
        });
        *font = font.clamped();
    }

    /// 设置弹窗里的「渲染后端」区块。
    ///
    /// 为什么要把这个开关暴露给用户：同一份二进制在不同机器上走的图形路径完全不同 ——
    /// 有独显的走 DX12/Vulkan，虚拟机与精简系统会退化到 WARP 软件光栅化，
    /// 远程桌面又是另一套。**画面撕裂 / 闪屏 / 白框**这类毛病高度依赖驱动实现，
    /// 换一个后端往往立刻就好，而这件事没有任何自动判据可言（画面对不对只有人眼知道）。
    /// 所以：给一个开关、记住选择、下次启动直接用。
    ///
    /// 改动写进 launch.json（eframe 状态目录里），由 `main` 在建窗之前读 ——
    /// 因此必须重启才生效，文案要说清楚。
    fn renderer_settings_ui(&mut self, ui: &mut egui::Ui) {
        use crate::launch::Backend;
        let theme = self.shared.theme;

        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "渲染后端");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // seg 在 right_to_left 里会倒着排：按自然顺序定义，绘制前反转，
                // 拿到下标再反转回来（与「界面缩放」那一处同一套写法）。
                let names: Vec<&str> = Backend::ALL.iter().rev().map(|b| b.label()).collect();
                let nat = Backend::ALL
                    .iter()
                    .position(|b| *b == self.launch_cfg.backend)
                    .unwrap_or(0);
                if let Some(n) = widgets::seg(ui, &theme, &names, Backend::ALL.len() - 1 - nat) {
                    let want = Backend::ALL[Backend::ALL.len() - 1 - n];
                    if want != self.launch_cfg.backend {
                        // 经 set_backend 走一遍磁盘：内存里这份是启动时读的，
                        // 之后 mark_running 改过盘上的内容，直接存回去会把它抹掉。
                        self.launch_cfg = crate::launch::set_backend(want);
                        self.pending_restart = Some(want);
                    }
                }
            });
        });
        ui.label(
            RichText::new("画面闪烁 / 撕裂 / 卡顿 / 打不开时换一个试试；「自动」由系统挑")
                .size(11.0)
                .color(theme.faint),
        );
        // 「重启后生效」必须配一个能当场重启的按钮。
        //
        // 后端只能在建窗**之前**决定（WGPU_BACKEND 是构造 NativeOptions 时读的），
        // 换后端天然要重启 —— 但只丢一句「重启后生效」等于把活儿推回给用户：
        // 他得自己关掉窗口再去开始菜单点开。而换后端本来就是个**试**的动作，
        // 试一次要手动重启一次，几乎没人会真的试完四个。
        if let Some(want) = self.pending_restart {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("已设为「{}」，重启后生效", want.label()))
                        .size(11.0)
                        .color(theme.fg_soft),
                );
                if widgets::primary_button(ui, &theme, "立即重启").clicked() {
                    self.restart_now();
                }
            });
        }
        // 当前**实际**在用的适配器：切后端做 A/B 对比时，这行是「切换真的生效了」
        // 的唯一证据 —— 锁定 Vulkan 但机器上没有，兜底照样会落回别的后端。
        if let Some(desc) = &self.gpu_desc {
            ui.label(
                RichText::new(format!("当前实际使用：{desc}"))
                    .family(FontFamily::Monospace)
                    .size(10.5)
                    .color(theme.fg_soft),
            );
            if self.gpu_software {
                ui.label(
                    RichText::new(
                        "正在软件渲染（无 GPU 加速）—— 画面发糊、拖动卡顿多半源于此：\
                         虚拟机请开启 3D 加速，物理机请安装显卡驱动；也可切换后端对比",
                    )
                    .size(10.5)
                    .color(theme.danger),
                );
                // 无 GPU 时每帧开销 ∝ 窗口像素数，这是唯一真正见效的软件侧手段。
                ui.label(
                    RichText::new(
                        "提速最有效的一招：把窗口拖小。软件渲染下开销与窗口像素数成正比，\
                         边长减半 ≈ 快 4 倍，尺寸会被记住。",
                    )
                    .size(10.5)
                    .color(theme.muted),
                );
            }
        }
        // Alt+Tab 卡顿缓解（仅 Windows/DX12 有意义）：把此前只能用 PowerShell 环境
        // 变量做的 A/B（WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT=none）收进设置，
        // 两次点击 + 重启即可对比。
        if cfg!(target_os = "windows") {
            ui.add_space(4.0);
            let on = self.launch_cfg.dx12_no_latency_wait;
            if widgets::pill_toggle(ui, &theme, on, "Alt+Tab 卡顿缓解（DX12，重启后生效）")
            {
                self.launch_cfg = crate::launch::set_dx12_no_latency_wait(!on);
                self.shared.toast(if !on {
                    "已开启：重启 Ferric 后生效；若无改善可再关掉对比"
                } else {
                    "已关闭：重启 Ferric 后恢复默认呈现节奏"
                });
            }
        }
        if let Some(b) = self.launch_cfg.last_good {
            ui.label(
                RichText::new(format!("上次成功启动使用：{}", b.label()))
                    .family(FontFamily::Monospace)
                    .size(10.5)
                    .color(theme.faint),
            );
        }
        // 上次启动失败的原因要给用户看见 —— 这是他判断该换成哪个的唯一依据。
        if let Some(e) = &self.launch_cfg.last_error {
            ui.label(
                RichText::new(format!("上次启动失败：{e}"))
                    .size(10.5)
                    .color(theme.danger),
            );
        }
    }

    /// 设置弹窗里的「软件更新」区块。
    ///
    /// 这里刻意把**服务器身份**摊开给用户看（地址 + 公钥指纹），因为自定义服务器的
    /// 配置存在用户可写的 ron 文件里、能被同用户的恶意进程静默改写 —— 可见性是用户
    /// 发现这件事的唯一途径。同理，非内置服务器时自动安装会被禁用。
    fn update_settings_ui(&mut self, ui: &mut egui::Ui) {
        use crate::updater::Phase;
        let theme = self.shared.theme;

        // —— 数据源：真服务端 / 演示数据。
        // 没烘入服务器的构建里，插件市场与更新原本是两块空白界面 ——
        // 演示数据让它们在任何构建下都能打开、能点、能演示（细节见 `mock` 模块头）。
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "数据源");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                use crate::source::SourcePref as P;
                const CHOICES: [(P, &str); 4] = [
                    (P::Auto, "自动"),
                    (P::Server, "服务器"),
                    (P::Github, "GitHub"),
                    (P::Mock, "演示"),
                ];
                let names: Vec<&str> = CHOICES.iter().rev().map(|(_, n)| *n).collect();
                let nat = CHOICES
                    .iter()
                    .position(|(v, _)| *v == self.source_pref)
                    .unwrap_or(0);
                if let Some(n) = widgets::seg(ui, &theme, &names, CHOICES.len() - 1 - nat) {
                    let want = CHOICES[CHOICES.len() - 1 - n].0;
                    if want != self.source_pref {
                        self.source_pref = want;
                        // 换了源，之前那次检查的结论就作废了
                        self.updater.phase = Phase::Idle;
                        self.shared.toast(match self.source() {
                            Some(s) if s.is_mock() => "已切换到演示数据（不联网）".to_owned(),
                            Some(crate::source::Source::Github(g)) => {
                                format!("已切换到 {}（只有更新，没有插件市场）", g.label())
                            }
                            Some(_) => "已切换到更新服务器".to_owned(),
                            None => "本构建没有烘入这个来源".to_owned(),
                        });
                    }
                }
            });
        });

        let Some(source) = self.source() else {
            ui.label(
                RichText::new("本构建未配置更新服务器；把数据源切到「演示」可预览完整流程")
                    .size(11.0)
                    .color(theme.faint),
            );
            return;
        };
        let builtin = source.is_builtin();

        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "软件更新");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let busy = self.updater.busy();
                ui.add_enabled_ui(!busy, |ui| {
                    if widgets::ghost_button(ui, &theme, "检查更新").clicked() {
                        let ctx = ui.ctx().clone();
                        self.updater.check(source.clone(), &ctx);
                    }
                });
                if self.server_override.is_some()
                    && widgets::ghost_button(ui, &theme, "恢复默认服务器").clicked()
                {
                    // 恢复默认必须同时重置地址与公钥 —— 它们是一个整体
                    self.server_override = None;
                    self.updater.phase = Phase::Idle;
                    self.shared.toast("已恢复内置更新服务器");
                }
            });
        });

        // 自动检查 + 后台下载。默认开：更新只有及时装上才有意义，
        // 但**安装始终要用户点**（见 `update_badge`），后台不替他做那个决定。
        ui.horizontal(|ui| {
            if widgets::pill_toggle(ui, &theme, self.auto_update, "自动检查并后台下载") {
                self.auto_update = !self.auto_update;
            }
            ui.label(
                RichText::new(if self.auto_update {
                    "下好后在顶栏出现「安装」按钮"
                } else {
                    "只在手动点「检查更新」时联网"
                })
                .size(11.0)
                .color(theme.faint),
            );
        });

        // 来源身份：地址 + 公钥指纹。指纹便于口头核对。
        match &source {
            crate::source::Source::Server(profile) => {
                ui.label(
                    RichText::new(format!("服务器  {}", profile.base_url))
                        .family(FontFamily::Monospace)
                        .size(10.5)
                        .color(theme.faint),
                );
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("公钥指纹 {}", profile.fingerprint()))
                            .family(FontFamily::Monospace)
                            .size(10.5)
                            .color(if builtin { theme.faint } else { theme.danger }),
                    );
                    // 指纹要能复制出去比对。肉眼比 16 个十六进制字符正是攻击者
                    // 指望的那一步 —— 相近的串很容易被看成一样。
                    if widgets::ghost_button(ui, &theme, "复制").clicked() {
                        ui.ctx().copy_text(profile.fingerprint());
                        self.shared.toast("指纹已复制");
                    }
                });

                // 核对：把「肉眼比对」换成机器比对。
                // 期望值从后台「服务器身份」页或运维那里拿，粘进来即可，
                // 大小写 / 空格 / 换行都容忍，但**位本身一位都不能差**。
                ui.horizontal(|ui| {
                    ui.label(RichText::new("核对指纹").size(10.5).color(theme.faint));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.fingerprint_input)
                            .hint_text("粘贴后台显示的指纹")
                            .desired_width(160.0),
                    );
                    let typed = self.fingerprint_input.trim();
                    if !typed.is_empty() {
                        let ok = profile.fingerprint_matches(typed);
                        ui.label(
                            RichText::new(if ok { "✓ 一致" } else { "✗ 不一致" })
                                .size(11.0)
                                .color(if ok { theme.ok } else { theme.danger }),
                        );
                    }
                });
                if !self.fingerprint_input.trim().is_empty()
                    && !profile.fingerprint_matches(self.fingerprint_input.trim())
                {
                    ui.label(
                        RichText::new(
                            "指纹对不上说明你连的不是那台服务器 —— 在弄清楚之前别装任何更新",
                        )
                        .size(11.0)
                        .color(theme.danger),
                    );
                }
            }
            crate::source::Source::Github(g) => {
                ui.label(
                    RichText::new(format!("GitHub  {}", g.repo))
                        .family(FontFamily::Monospace)
                        .size(10.5)
                        .color(if builtin { theme.faint } else { theme.danger }),
                );
                // GitHub 这条路没有传输公钥可核对（TLS 由 GitHub 提供），
                // 但**执行授权仍然只认离线签名** —— 这一点要说清楚，
                // 否则用户会以为换了源就等于降级了安全性
                ui.label(
                    RichText::new("传输由 GitHub 的 TLS 保证；安装包仍须通过内置发布公钥验签")
                        .size(11.0)
                        .color(theme.faint),
                );
                ui.label(
                    RichText::new("此来源只提供应用更新，没有插件市场")
                        .size(11.0)
                        .color(theme.faint),
                );
            }
            crate::source::Source::Mock => {
                ui.label(
                    RichText::new("演示数据 · 不联网；下载进度是模拟的，安装不会真的执行")
                        .size(11.0)
                        .color(theme.danger),
                );
            }
        }
        if !builtin && !source.is_mock() {
            ui.label(
                RichText::new("⚠ 更新源已被修改，自动安装已禁用（只会提示新版本）")
                    .size(11.0)
                    .color(theme.danger),
            );
        }

        // 上次成功检查时间：长期检查不成功要让用户看见 ——
        // 这是对抗「中间人直接丢包压制更新」的唯一手段。
        match self.updater.last_ok {
            Some(t) => {
                let days = t.elapsed().map(|d| d.as_secs() / 86400).unwrap_or(0);
                let txt = if days == 0 {
                    "上次成功检查：今天".to_owned()
                } else {
                    format!("上次成功检查：{days} 天前")
                };
                ui.label(RichText::new(txt).size(11.0).color(if days >= 14 {
                    theme.danger
                } else {
                    theme.faint
                }));
            }
            None => {
                ui.label(
                    RichText::new("尚未成功检查过更新")
                        .size(11.0)
                        .color(theme.faint),
                );
            }
        }

        // 自定义 GitHub 更新源。只有一个 `owner/repo`，没有公钥要配 ——
        // 传输交给 GitHub 的 TLS，执行授权仍旧只认内置的发布验签公钥，
        // 所以改这里**不会**放宽任何执行门槛，最坏结果是「下下来的包验不过签」。
        // 「恢复默认」要把草稿一起清掉，但草稿是从 `self.github_draft` 借出来的，
        // 不能在借用还活着的时候赋值。用一个标志把清理挪到闭包之后。
        let mut clear_github_draft = false;
        ui.collapsing(
            RichText::new("GitHub 更新源").size(11.5).color(theme.faint),
            |ui| {
                let draft = self.github_draft.get_or_insert_with(|| {
                    self.github_override
                        .as_ref()
                        .map(|g| g.repo.clone())
                        .or_else(|| crate::github::GithubSource::builtin().map(|g| g.repo))
                        .unwrap_or_default()
                });
                ui.label(
                    RichText::new("仓库（owner/repo）")
                        .size(10.5)
                        .color(theme.faint),
                );
                ui.add(egui::TextEdit::singleline(draft).desired_width(f32::INFINITY));
                ui.label(
                    RichText::new("发布页里必须有 manifest.json 附件（含各平台的 sha256 与签名）")
                        .size(10.5)
                        .color(theme.faint),
                );
                ui.horizontal(|ui| {
                    if widgets::ghost_button(ui, &theme, "应用").clicked() {
                        let candidate = crate::github::GithubSource {
                            repo: draft.trim().to_owned(),
                        };
                        match candidate.validate() {
                            Ok(()) => {
                                let is_builtin = candidate.is_builtin();
                                self.github_override = (!is_builtin).then_some(candidate);
                                self.updater.phase = Phase::Idle;
                                self.shared.toast(if is_builtin {
                                    "与内置仓库一致，已按内置处理"
                                } else {
                                    "已切换 GitHub 更新源；自动安装已禁用，仅提示新版本"
                                });
                            }
                            Err(e) => self.shared.toast(format!("仓库无效：{e}")),
                        }
                    }
                    if self.github_override.is_some()
                        && widgets::ghost_button(ui, &theme, "恢复默认仓库").clicked()
                    {
                        self.github_override = None;
                        clear_github_draft = true;
                        self.updater.phase = Phase::Idle;
                        self.shared.toast("已恢复内置 GitHub 仓库");
                    }
                });
            },
        );
        if clear_github_draft {
            self.github_draft = None;
        }

        // 自定义更新服务器。地址与公钥**一起改**，禁止只改其一 ——
        // 「只把地址指向我的服务器」是最省事的一种攻击，数据结构上就不让它成立。
        ui.collapsing(
            RichText::new("自定义更新服务器")
                .size(11.5)
                .color(theme.faint),
            |ui| {
                // 草稿初值取当前服务器；演示模式下没有服务器，就从空开始填
                let (url, key) =
                    self.server_draft.get_or_insert_with(
                        || match crate::net::ServerProfile::builtin() {
                            Some(p) => (p.base_url, p.pubkey),
                            None => (String::new(), String::new()),
                        },
                    );
                ui.label(RichText::new("服务器地址").size(10.5).color(theme.faint));
                ui.add(egui::TextEdit::singleline(url).desired_width(f32::INFINITY));
                ui.label(
                    RichText::new("SM2 公钥（04 开头 130 个 hex 字符，须完整粘贴）")
                        .size(10.5)
                        .color(theme.faint),
                );
                ui.add(
                    egui::TextEdit::multiline(key)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );

                // 边粘边算指纹：**换服务器之前**才是核对指纹最该发生的时刻。
                // 等点完「应用」再看，那时信任已经交出去了。
                let candidate_key: String = key.split_whitespace().collect();
                if !candidate_key.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "这把公钥的指纹 {}",
                            crate::net::fingerprint_of(&candidate_key)
                        ))
                        .family(FontFamily::Monospace)
                        .size(10.5)
                        .color(theme.muted),
                    );
                    ui.label(
                        RichText::new("请与对方公布的指纹逐位核对后再应用")
                            .size(10.5)
                            .color(theme.faint),
                    );
                }

                ui.horizontal(|ui| {
                    if widgets::ghost_button(ui, &theme, "应用").clicked() {
                        let candidate = crate::net::ServerProfile {
                            base_url: url.trim().to_owned(),
                            pubkey: key.split_whitespace().collect::<String>(),
                        };
                        match candidate.validate() {
                            Ok(()) => {
                                let is_builtin = candidate.is_builtin();
                                self.server_override = (!is_builtin).then_some(candidate);
                                self.updater.phase = Phase::Idle;
                                self.shared.toast(if is_builtin {
                                    "与内置服务器一致，已按内置处理"
                                } else {
                                    "已切换更新源；自动安装已禁用，仅提示新版本"
                                });
                            }
                            Err(e) => self.shared.toast(format!("配置无效：{e}")),
                        }
                    }
                });
            },
        );

        // 状态。注意「检查失败」与「已是最新」必须可区分 ——
        // 否则中间人只要丢包就能把自己伪装成「你已经是最新版」。
        match self.updater.phase.clone() {
            Phase::Idle => {}
            Phase::Checking => {
                ui.label(RichText::new("正在检查…").size(11.5).color(theme.faint));
            }
            Phase::UpToDate => {
                widgets::status_line(ui, &theme, true, "已是最新版本");
            }
            Phase::Failed(e) => {
                widgets::status_line(ui, &theme, false, &format!("检查失败：{e}"));
            }
            Phase::Downloading { done, total } => {
                let pct = done
                    .checked_mul(100)
                    .and_then(|x| x.checked_div(total))
                    .unwrap_or(0);
                ui.label(
                    RichText::new(format!("下载中 {pct}%（{done}/{total} 字节）"))
                        .size(11.5)
                        .color(theme.faint),
                );
            }
            Phase::Available(info) => {
                widgets::status_line(
                    ui,
                    &theme,
                    true,
                    &format!("发现新版本 v{}（build {}）", info.version, info.build),
                );
                if info.force {
                    // 强制标记只作提示，**不做不可逃逸的模态框**：force 由本地重算，
                    // 但服务端的 min_supported_build 可能被误配或被入侵者拉高，
                    // 把用户锁死在打不开的状态是更坏的失败模式。
                    ui.label(
                        RichText::new("此为强制更新：当前版本已低于服务端声明的最低支持版本")
                            .size(11.0)
                            .color(theme.danger),
                    );
                }
                if !info.notes.trim().is_empty() {
                    ui.label(RichText::new(&info.notes).size(11.0).color(theme.faint));
                }
                ui.horizontal(|ui| {
                    // 演示源也允许「下载」——它下的是模拟进度，且永远不会被执行。
                    // 自定义源则只通知：那个地址可能是用户被诱导改的。
                    if source.allows_auto_download() {
                        if widgets::ghost_button(ui, &theme, "下载并校验").clicked() {
                            let ctx = ui.ctx().clone();
                            self.updater.download(source.clone(), info.clone(), &ctx);
                        }
                    } else {
                        ui.label(
                            RichText::new("请自行前往更新源手动下载安装")
                                .size(11.0)
                                .color(theme.faint),
                        );
                    }
                });
            }
            Phase::Ready { info, file } => {
                widgets::status_line(
                    ui,
                    &theme,
                    true,
                    &format!("v{} 已下载，签名与校验和均已通过", info.version),
                );
                // 各平台行为并不一致，文案要如实说明
                let hint = if !source.allows_install() {
                    "演示数据：不会真的执行安装程序"
                } else {
                    match std::env::consts::OS {
                        "windows" => "将启动安装程序并退出 Ferric",
                        "macos" => "将打开安装包，需你手动完成安装",
                        _ => "将交给系统的软件安装器，可能需要授权",
                    }
                };
                ui.label(RichText::new(hint).size(11.0).color(theme.faint));
                if widgets::ghost_button(ui, &theme, "立即安装").clicked() {
                    if source.allows_install() {
                        match crate::updater::launch(&file) {
                            Ok(()) => {
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            Err(e) => {
                                self.updater.phase = Phase::Failed(e);
                            }
                        }
                    } else {
                        self.shared
                            .toast("演示模式：不会真的执行安装程序（换成真实更新源即可）");
                    }
                }
            }
        }
    }

    /// 底部提示条。**按时间到期，且只在到期那一刻醒一次**。
    ///
    /// 这里曾是 `frames_left` 倒数 + 每帧 `ctx.request_repaint()`：提示条在的
    /// 每一刻，整窗都在按显示器刷新率重画。提示条本身是静止的（没有淡入淡出、
    /// 没有进度），一帧画完就不该再动 —— 需要的只是「到点了再醒一次把它擦掉」。
    /// 帧数计时的正反馈见 [`crate::tool::TOAST_TTL`]。
    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let theme = self.shared.theme;
        let now = std::time::Instant::now();
        self.shared.toasts.retain(|t| t.until > now);
        if let Some(t) = self.shared.toasts.last() {
            egui::Area::new(egui::Id::new("toast"))
                .anchor(Align2::CENTER_BOTTOM, [0.0, -30.0])
                .show(ctx, |ui| {
                    Frame::NONE
                        .fill(theme.fg)
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(16, 9))
                        .show(ui, |ui| {
                            ui.label(RichText::new(&t.msg).color(theme.bg).size(13.0));
                        });
                });
            // 到期那一刻醒一次即可 —— 多次调用会被 egui 合并成最早的那个醒点。
            ctx.request_repaint_after(t.until - now);
        }
    }

    /// 更新就绪后自动弹出的模态更新框。
    ///
    /// 只在「已下载并通过 sha256 + 签名三重校验」后打开（`Tick::ReadyToInstall` 置位
    /// `update_dialog_open`）。框里给出版本与更新说明，主按钮是「立即安装」——点击即
    /// 覆盖安装当前版本（拉起安装程序并退出本进程）。「稍后」只是收起框，顶栏的
    /// 「安装 vX」按钮仍在，随时可装。
    fn update_dialog_ui(&mut self, ctx: &egui::Context) {
        if !self.update_dialog_open {
            return;
        }
        let theme = self.shared.theme;
        let phase = self.updater.phase.clone();
        let source = self.source();
        let allows_install = source.as_ref().is_some_and(|s| s.allows_install());

        // 只有「就绪」才有安装动作可做；其它状态（失败 / 已是最新）不该有这框。
        let (info, file) = match &phase {
            crate::updater::Phase::Ready { info, file } => (info.clone(), file.clone()),
            _ => {
                self.update_dialog_open = false;
                return;
            }
        };

        enum Action {
            Dismiss,
            Install,
        }
        // 闭包里只登记意图，出来后统一改 `self`（`Modal::show` 的闭包是 FnOnce，
        // 直接在里面改 `self` 会跟 `&mut self` 的借用打架）。
        let mut action: Option<Action> = None;

        let frame = Frame::NONE
            .fill(theme.bg)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(22))
            .stroke(Stroke::new(1.0, theme.border_2));

        let modal = egui::Modal::new(egui::Id::new("update-dialog"))
            .backdrop_color(Color32::from_black_alpha(120))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(icons::text(icons::REFRESH_CW, 18.0, theme.accent));
                        ui.label(
                            RichText::new(format!("发现新版本 v{}", info.version))
                                .size(16.0)
                                .strong()
                                .color(theme.fg),
                        );
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "当前 v{}（构建 {}）→ v{}（构建 {}）",
                            crate::updater::my_version(),
                            crate::updater::my_build(),
                            info.version,
                            info.build,
                        ))
                        .size(11.0)
                        .color(theme.muted),
                    );
                    if info.force {
                        ui.label(
                            RichText::new("强制更新：当前版本已低于声明的最低支持版本")
                                .size(11.0)
                                .color(theme.danger),
                        );
                    }
                    if let Some(badge) = source.as_ref().and_then(|s| s.badge()) {
                        ui.label(RichText::new(&badge).size(10.5).color(theme.faint));
                    }

                    if !info.notes.trim().is_empty() {
                        ui.add_space(8.0);
                        ScrollArea::vertical()
                            .max_height(160.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&info.notes).size(11.5).color(theme.fg_soft),
                                );
                            });
                    }

                    ui.add_space(12.0);
                    let hint = if !allows_install {
                        "该更新源不会真的执行安装（演示 / 自定义源）"
                    } else {
                        match std::env::consts::OS {
                            "windows" => "点击「立即安装」将覆盖当前版本并退出 Ferric",
                            "macos" => "点击「立即安装」将打开安装包，需手动完成",
                            _ => "点击「立即安装」将交给系统的软件安装器",
                        }
                    };
                    ui.label(RichText::new(hint).size(11.0).color(theme.muted));

                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::ghost_button(ui, &theme, "稍后").clicked() {
                            action = Some(Action::Dismiss);
                        }
                        if widgets::primary_button(ui, &theme, "立即安装")
                            .on_hover_text(if allows_install {
                                "覆盖安装当前版本（将退出 Ferric）"
                            } else {
                                "演示 / 自定义更新源不会真的安装"
                            })
                            .clicked()
                        {
                            action = Some(Action::Install);
                        }
                    });
                });
            });

        // 用户明确点了「立即安装」就优先安装，别被同一帧的 Esc / 背景点击覆盖成「稍后」。
        if action.is_none() && modal.should_close() {
            action = Some(Action::Dismiss);
        }

        match action {
            Some(Action::Dismiss) => self.update_dialog_open = false,
            Some(Action::Install) => {
                self.update_dialog_open = false;
                if allows_install {
                    match crate::updater::launch(&file) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            self.updater.phase = crate::updater::Phase::Failed(e.clone());
                            self.shared.toast(e);
                        }
                    }
                } else {
                    self.shared.toast("演示 / 自定义更新源不会真的执行安装程序");
                }
            }
            None => {}
        }
    }
}

/// 内容区居中列的 (左右留白, 列宽)：超过阈值才居中，否则贴边 24px。
fn content_metrics(avail: f32) -> (f32, f32) {
    let target = 1080.0;
    let side = if avail > target + 48.0 {
        (avail - target) / 2.0
    } else {
        24.0
    };
    (side, (avail - side * 2.0).max(320.0))
}

/// 分组图标（Lucide 字形）。
fn group_icon(group: &str) -> char {
    match group {
        "收藏" => icons::HEART,
        "JSON" => icons::CODE,
        "对比" => icons::GIT_COMPARE,
        "转换" => icons::CLOCK,
        "SQL" => icons::DATABASE,
        "生成" => icons::CREDIT_CARD,
        "加密" => icons::LOCK,
        "文本" => icons::TERMINAL,
        "插件" => icons::BOX,
        _ => icons::BOX,
    }
}

impl FerricApp {
    /// 调试用自拍：设了 `FERRIC_SCREENSHOT=<路径.ppm>` 就在跑满若干帧后把窗口内容
    /// 存成 PPM 并退出。默认（不设该变量）完全不生效。
    ///
    /// 写 PPM 而不是 PNG：格式就是一行头加一串 RGB 字节，不用为一个调试开关引入
    /// 图像编码依赖；任何看图工具都能打开。
    ///
    /// 为什么需要它：改完界面想确认「到底长什么样」，在受限环境里往往拿不到 ——
    /// 比如 GNOME 会拒绝非授权的 D-Bus 截图、XWayland 下抓 X11 根窗口也会失败。
    /// 让应用自己交出渲染结果是唯一不依赖桌面环境配合的办法，顺带把字体、主题、
    /// 缩放这些真实渲染路径也一并走了一遍。CI 里同样可用。
    ///
    /// 注意它只能拍**静态画面**：想拍「选中之后」这类交互结果是做不到的 ——
    /// egui 的点击判定读的是 `InputState::pointer`，那些字段是私有的，
    /// 往事件队列里塞 PointerButton 不会让 egui 认为发生过点击。
    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Ok(path) = std::env::var("FERRIC_SCREENSHOT") else {
            return;
        };
        // 先收上一帧请求的结果
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let (w, h) = (img.width(), img.height());
            let mut buf = format!("P6\n{w} {h}\n255\n").into_bytes();
            buf.extend(img.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b()]));
            let ok = std::fs::write(&path, &buf).is_ok();
            eprintln!(
                "[screenshot] {} {w}x{h} → {path}",
                if ok { "已保存" } else { "保存失败" }
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // 等界面稳定（字体装载、主题、布局都落定）后再拍
        self.shot_frames += 1;
        // 可选：拍照前先往输入事件里塞一个 Ctrl+A。本函数在各视图绘制**之前**运行，
        // 所以这一帧稍后绘制的编辑器会像收到真实按键一样处理它 —— 于是截图里
        // 能看到真实的选区渲染，而不是靠猜。
        if self.shot_frames == 45 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
    }
}

impl FerricApp {
    /// Windows 呈现卫生：缓解两类由 DX12 flip-model 呈现引起的观感问题。
    /// 其余平台整个函数是空操作。
    ///
    /// 1. **Alt+Tab 切回时闪一下旧画面**：DWM 合成窗口用的是交换链里最后 present
    ///    的那一帧，而 winit 的 Windows 后端不产生 `Occluded` 事件 —— 失焦期间没有
    ///    任何重绘，那帧可以旧到任意程度；切回后新帧又要等下一轮事件循环才出。
    ///    两手一起做：焦点上升沿立即请求重绘，把旧帧停留压到一帧；失焦期间保持
    ///    500ms 一次的低频心跳，让交换链里的「旧帧」与走时的画面始终一致 ——
    ///    切回来看到的内容没有差异，就谈不上闪。
    /// 2. **拖动窗口时整窗「发花」**：flip-model 的 present 与窗口移动天然不同步，
    ///    Fifo（等 vblank）让画面恒定落后窗口约一帧；无边框窗口整个客户区都是内容，
    ///    错一帧就是全屏残影。这个根子已在 `main.rs` 层面解决：Windows 上表面配置
    ///    改用 Mailbox（单帧队列，present 不再排队等 vblank），拖动期间无需再切换。
    ///    这里只负责「移动中不重绘」—— 内容相对客户区没变，一帧不画就一帧不会错位。
    fn present_hygiene(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !cfg!(target_os = "windows") {
            return;
        }

        // —— 焦点上升沿立即重绘 + 失焦低频心跳。
        // 心跳在**最小化**时豁免：最小化的窗口不参与 DWM 合成，恢复时本来就有
        // 重绘事件，没有「旧帧先亮相」的问题 —— 那 2fps 就纯属烧电了。
        let focused = ctx.input(|i| i.focused);
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        if focused && !self.was_focused {
            ctx.request_repaint();
        }
        if !focused && !minimized {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        self.was_focused = focused;

        // —— 拖动 / 交互式移动检测：外沿左上角变了就算在动。
        let pos = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min));
        let now = std::time::Instant::now();
        if let (Some(p), Some(q)) = (pos, self.last_outer_pos) {
            if (p - q).length_sq() > 0.25 {
                self.last_moved_at = Some(now);
            }
        }
        if pos.is_some() {
            self.last_outer_pos = pos;
        }
        let moving = self
            .last_moved_at
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(300));
        self.fast_present = moving;

        // 移动期间**什么都不做**：不重绘、不换呈现模式。
        //
        // 这里原先是「切 AutoNoVsync + 每帧 request_repaint」，想法是「一直出新帧，
        // 残影窗口最短」。在有 GPU 的机器上那没问题，但靠 CPU 光栅化时它是反效果：
        // 实测拖动期间应用冲到 **1193 fps**，而这台机器只显示得出约 30fps ——
        // 多出的帧全积在提交队列里，**屏幕上那一帧是很多帧以前渲染的**，
        // 对应的正是旧的窗口位置。越努力出帧，画面反而落后窗框越多。
        //
        // 关键在于：**拖动时窗口内容压根没变**。界面是相对客户区绘制的，位置由
        // 系统改，DWM 直接把已合成好的那一面搬到新位置即可 —— 一帧不画，
        // 就一帧都不会错位，也不会有 ALLOW_TEARING 撕出来的半截画面。
        // 有内容真变了（悬停、动画）时 egui 照常自己请求重绘，走正常节流即可。
        //
        // 保留 `fast_present` 只为让 [`Self::throttle_soft_render`] 能看到「在移动」，
        // 不再据此换表面配置：换配置本身要重建交换链，拖动中途做这件事只会添乱。

        let _ = frame;
    }
}

impl eframe::App for FerricApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.shared.theme.bg;
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.persist());
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.throttle_soft_render();
        let t_frame_start = std::time::Instant::now();
        let ctx = root_ui.ctx().clone();
        let ctx = &ctx;
        self.present_hygiene(ctx, frame);
        // 稳定出帧 = 本次启动成功。把当前渲染后端记成 last_good 并清掉「正在尝试」
        // 标记，否则下次启动会以为上次是崩在启动路上的（见 launch::plan）。
        if self.frames < STABLE_FRAMES {
            self.frames += 1;
            if self.frames == STABLE_FRAMES {
                let out = crate::launch::mark_running(self.gpu_software);
                if out != crate::launch::Outcome::default() {
                    self.launch_cfg = crate::launch::load();
                }
                // 锁定的后端被证明不可用时会被改回「自动」—— 必须让用户知道，
                // 否则设置里显示的选项和实际行为对不上。
                if let Some(b) = out.lock_dropped {
                    self.shared.toast(format!(
                        "渲染后端 {} 在本机不可用，已改回「自动」",
                        b.label()
                    ));
                }
                // 只拿到软件渲染、且还有没试过的后端 —— 自愈已经排好了下一个，
                // 但它要等重启才生效。用户现在正卡着，得给他一个当场能点的去处，
                // 而不是让他自己琢磨「要不要重启一下试试」。
                if let Some(next) = out.will_retry_with {
                    self.slow_render_retry = Some(next);
                }
                // 启动诊断：稳定出帧后一次性把可量化的内部状态写到 startup.log。
                // 「600M+」是观察值，根因在 wgpu runtime / 字体 atlas / 撤销栈 / persistence
                // 哪一坨要靠这份拆分去对 —— 一行数对得齐，就知道下一刀该砍谁。
                startup_diag(self);
            }
        }
        // 30 秒内存采样：每帧轮询，到点落盘并提示用户。
        // 与 startup_diag 同一个上下文：可以拿到 Persist 的字节视图。
        self.poll_mem_recorder(ctx);
        self.debug_screenshot(ctx);
        // 跟随系统模式下与操作系统深浅色保持同步（含启动首帧与运行中切换）。
        self.sync_theme(ctx);

        // 更新进度轮询必须在**外壳顶层**，不能挂在某个视图里 ——
        // 更新是全局的，挂在视图里会导致用户不切到那页就永远收不到结果。
        self.updater.poll(ctx);
        // 插件市场视图需要知道数据源；每帧同步，改了设置立刻生效
        let source = self.source();
        self.shared.source = source.clone();

        // 后台流水线：到点自动检查 → 发现新版自动下载 → 就绪后提示一次。
        // 只调度不安装 —— 安装会关掉正在用的应用，那必须由用户点。
        let stale = self.update_check_is_stale();
        let tick = self
            .updater
            .tick(ctx, source.as_ref(), self.auto_update, stale);
        if let crate::updater::Tick::ReadyToInstall { .. } = tick {
            // 更新已下载并校验通过：弹更新框（见 `update_dialog_ui`），
            // 而不是只靠顶栏那颗小按钮 —— 用户可能根本没注意到它。
            self.update_dialog_open = true;
        }
        // 成功检查过就记下时间，供下次启动节流。
        // 比大小而不是「只记一次」：只记一次的话时间戳永远停在第一次，
        // 过了间隔之后每启动一次都会重新查。
        if let Some(t) = self.updater.last_ok {
            let secs = t
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            // 写成 match 而不是 `map_or` / `is_none_or`：前者会被新版 clippy 判为
            // 「该用 is_none_or」，后者要 Rust 1.82，而本仓库声明的 MSRV 是 1.80。
            let newer = match self.last_update_check {
                Some(prev) => secs > prev,
                None => true,
            };
            if newer {
                self.last_update_check = Some(secs);
            }
        }

        // Ctrl+K 聚焦搜索框
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::K)) {
            self.focus_search = true;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.settings_open = false;
        }

        // 无边框窗口的边 / 角缩放。
        chrome::handle_resize(ctx);

        let theme = self.shared.theme;
        let root = Frame::NONE.fill(theme.bg);

        CentralPanel::default().frame(root).show(root_ui, |ui| {
            Panel::top("titlebar")
                .exact_size(TITLE_BAR_HEIGHT)
                .frame(Frame::NONE.fill(theme.titlebar))
                .show_separator_line(false)
                .show(ui, |ui| {
                    chrome::title_bar_content(ui, &theme);
                });

            self.slow_render_banner(ui);

            let rail_resp = Panel::left("rail")
                .resizable(true)
                .min_size(RAIL_MIN)
                .max_size(RAIL_MAX)
                .default_size(self.rail_width)
                .frame(
                    Frame::NONE
                        .fill(theme.rail)
                        .inner_margin(Margin::symmetric(12, 6)),
                )
                .show_separator_line(false)
                .show(ui, |ui| {
                    // 右侧竖分隔线
                    ui.painter().vline(
                        ui.max_rect().right() + 12.0,
                        ui.max_rect().y_range(),
                        Stroke::new(1.0_f32, theme.border),
                    );
                    self.rail_ui(ui);
                });
            self.rail_width = rail_resp.response.rect.width();

            CentralPanel::default()
                .frame(Frame::NONE.fill(theme.bg))
                .show(ui, |ui| {
                    Panel::top("topbar")
                        .exact_size(60.0)
                        .frame(Frame::NONE.fill(theme.bg))
                        .show_separator_line(false)
                        .show(ui, |ui| {
                            self.topbar_ui(ui);
                        });
                    CentralPanel::default()
                        .frame(Frame::NONE.fill(theme.bg))
                        .show(ui, |ui| {
                            self.content_body(ui);
                        });
                });
        });

        // 全局窗口 1px 边框
        self.window_border_ui(ctx);

        self.settings_ui(ctx);
        self.toasts_ui(ctx);
        self.update_dialog_ui(ctx);

        // 插件热加载放在**所有视图渲染完之后**：市场视图正是 self.tools 里的一员，
        // 在它的 ui() 里改这个向量是不可能的（元素正被借着）。
        if std::mem::take(&mut self.shared.reload_plugins) {
            self.reload_plugins();
        }

        // 重启同理，必须等本帧画完：`do_restart` 会存盘并关窗，
        // 在绘制中途做这些事等于在半张画面上拔插头。
        if std::mem::take(&mut self.want_restart) {
            self.do_restart(ctx, frame);
        }

        perf_probe(ctx, t_frame_start);
    }
}

/// 重绘归因探针：`FERRIC_PERF=1` 时每秒往 `startup.log` 写一行
/// 「这一秒画了多少帧、每帧多久、最后一帧是谁要求重绘的」。默认完全不生效。
///
/// # 为什么需要它
///
/// 「卡」这件事在 egui 里几乎总是同一个形状：**某处在无谓地要求重绘**。
/// egui 没有局部重绘，一帧就是整窗重新排版 + 三角化 + 光栅化；有 GPU 时
/// 60fps 空转看不出来，靠 CPU 画（Windows 的 WARP、虚拟机里的 llvmpipe）时
/// 就是整机发卡。而「谁要求的」光看代码是猜不出来的 —— `repaint_causes()`
/// 会直接给出请求方的文件与行号。历史上两次卡顿归因都靠它当场锁定：
/// 更新下载的进度回调、以及提示条按帧计时。
///
/// 写文件而不是 `eprintln!`：发行版是 `windows_subsystem = "windows"`，
/// stderr 没有任何去处 —— 而恰恰是 Windows 用户最需要交出这份数据。
fn perf_probe(ctx: &egui::Context, t0: std::time::Instant) {
    use std::cell::RefCell;
    use std::sync::OnceLock;

    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("FERRIC_PERF").is_some()) {
        return;
    }
    thread_local! {
        /// (本轮起点, 帧数, 累计 ui() 耗时 ms, 最慢一帧 ms)
        static ACC: RefCell<(std::time::Instant, u32, f64, f64)> =
            RefCell::new((std::time::Instant::now(), 0, 0.0, 0.0));
    }
    let dt = t0.elapsed().as_secs_f64() * 1000.0;
    ACC.with(|s| {
        let (start, frames, total, worst) = &mut *s.borrow_mut();
        *frames += 1;
        *total += dt;
        *worst = worst.max(dt);
        let secs = start.elapsed().as_secs_f64();
        if secs < 1.0 {
            return;
        }
        let causes: Vec<String> = ctx
            .repaint_causes()
            .iter()
            .map(|c| format!("{}:{}", c.file.rsplit('/').next().unwrap_or(c.file), c.line))
            .collect();
        crate::launch::log(&format!(
            "[perf] {:.1} fps · ui() 均 {:.2}ms 峰 {:.2}ms · 重绘请求方：{}",
            *frames as f64 / secs,
            *total / *frames as f64,
            worst,
            causes.join(", ")
        ));
        *start = std::time::Instant::now();
        *frames = 0;
        *total = 0.0;
        *worst = 0.0;
    });
}

/// 启动后一次性诊断报告：把可量化的内部状态（持久化、工具、草稿、字体）写到 startup.log。
///
/// 触发时机：第三帧稳定出帧后（与 `mark_running` 同帧）。跑得太早（首帧）会让
/// 字体纹理 / 撤销栈尚未完成首次提交；太晚（运行中）会丢掉**冷启动内存**的快照，
/// 而那正是「600M+」现象发生的位置。
///
/// 这里**不**测进程 RSS：`windows-sys` / `libc` 都是新增依赖，且 RSS 自身在
/// ARM Windows 等环境里与「任务管理器 · 内存(私有工作集)」并不完全对应
/// （working set 还含共享 / 已映射未触页面）。用户想看 RSS 直接看任务管理器
/// 更准。这里只把**我们可以**优化的子系统量化列出来，差值留给 RSS。
///
/// 输出格式（单行，便于 `grep` / 排序）：
///
/// ```text
/// [diag] tools=N persist=XB drafts=NB (top3) fonts=XB (design=XB cjk=XB) adapter=...
/// ```
///
/// - `tools`：工具数量（含插件）。10 个工具常驻，每个持自己的 `Vec<Rope>` /
///   撤销栈 / 折叠树 —— 单看这个数没意义，但配合 `drafts` 总和能给出量级。
/// - `persist`：把 `Persist` 序列化成 JSON 后的字节数 —— **这就是 eframe
///   每次 save 写盘的体积**，会原样落到 `app.ron`。
/// - `drafts`：各工具草稿的字节数（按字节降序，前 3 名列出，其余求和）。
///   JSON 工具的 Rope 草稿通常是最大头；一个 5MB JSON = ~5MB 草稿。
/// - `fonts`：内嵌字体字节总和（设计字体 + Lucide 图标 + CJK 系统字体估算）。
///   **CJK 字体通常是这行的最大头**（8~30MB）；软渲染与 wgpu 路径都会吃，
///   只是 wgpu 路径被 D3D12 runtime 衬得没那么显眼 —— 砍掉 CJK 之前先看这行。
fn startup_diag(app: &FerricApp) {
    // 持久化：与 eframe 写盘走同一份序列化，体积上下一致。
    // 故意走 JSON（ferric-core 不必引 ron），字节数只差 ~5%，对拆分够用。
    let persist = app.persist();
    let persist_bytes = serde_json::to_vec(&persist).map(|v| v.len()).unwrap_or(0);

    // 草稿：按字节降序，前 3 名列出，其余合并到 "other"。
    let mut drafts: Vec<(&str, usize)> = persist
        .drafts
        .iter()
        .map(|(k, v)| (k.as_str(), v.len()))
        .collect();
    drafts.sort_by_key(|b| std::cmp::Reverse(b.1));
    let top: Vec<String> = drafts
        .iter()
        .take(3)
        .map(|(k, b)| format!("{}={}", k, fmt_bytes(*b)))
        .collect();
    let other_sum: usize = drafts.iter().skip(3).map(|(_, b)| *b).sum();
    let drafts_total: usize = drafts.iter().map(|(_, b)| *b).sum();
    let mut drafts_str = top.join(",");
    if other_sum > 0 {
        if !drafts_str.is_empty() {
            drafts_str.push(',');
        }
        drafts_str.push_str(&format!("other={}", fmt_bytes(other_sum)));
    }

    let design_font_bytes: usize = crate::fonts::embedded_bytes();
    let cjk_bytes = crate::fonts::cjk_bytes();
    let font_bytes = design_font_bytes + cjk_bytes;

    crate::launch::log(&format!(
        "[diag] tools={} persist={} drafts={} ({}) fonts={} (design={} cjk={}) adapter={}",
        app.tools.len(),
        fmt_bytes(persist_bytes),
        fmt_bytes(drafts_total),
        if drafts_str.is_empty() {
            "-"
        } else {
            &drafts_str
        },
        fmt_bytes(font_bytes),
        fmt_bytes(design_font_bytes),
        fmt_bytes(cjk_bytes),
        app.gpu_desc.as_deref().unwrap_or("?"),
    ));
}

impl FerricApp {
    /// 用户在「关于」页点"记录 30 秒内存"后启动录制。
    /// data_dir 拿不到（极冷启动场景）就静默失败——按 mem.rs 的契约，
    /// 拿不到路径就不开采样，不让按钮变成"点了没反应"。
    fn start_mem_recording(&mut self) {
        if self.mem_recorder.is_some() {
            return; // 录制中重复点不重启，提示在 UI 上显示。
        }
        let Some(dir) = crate::launch::data_dir() else {
            self.shared.toast("无法定位数据目录，录制未开始");
            return;
        };
        let backend = self.launch_cfg.backend.label();
        self.mem_recorder = Some(crate::mem::MemoryRecorder::start(&dir, backend));
    }

    /// 30 秒内存采样的轮询钩子：每帧检查 recorder 的到期状态。
    /// 落到点就把 `memory.log` 写出去、toast 通知用户、清除 recorder。
    fn poll_mem_recorder(&mut self, _ctx: &egui::Context) {
        use crate::mem::TickOutcome;
        // 先取走需要的快照数据，避免与下面 mem_recorder 的可变借用重叠。
        let (persist_bytes, drafts_bytes) = persist_size_split(&self.persist());
        let Some(rec) = self.mem_recorder.as_mut() else {
            return;
        };
        match rec.tick(persist_bytes, drafts_bytes) {
            TickOutcome::Pending | TickOutcome::Sampled(_) => {
                // 录制中无需重绘提示——UI 上按钮已经显示「正在记录… X / 30」。
            }
            TickOutcome::Finished(_) => {
                // take() 把 recorder 拿掉，避免下一次录制开始前多调一次 finish。
                if let Some(rec) = self.mem_recorder.take() {
                    match rec.finish() {
                        Ok(()) => self.shared.toast("已保存 memory.log（30 秒采样）"),
                        Err(e) => self.shared.toast(format!("保存 memory.log 失败：{e}")),
                    }
                }
            }
        }
    }
}

/// `(persist 序列化字节, drafts 总字节)`：和 `startup_diag` 同口径，
/// 抽取出来给 `poll_mem_recorder` 复用，避免 `tick()` 里再写一遍序列化。
fn persist_size_split(p: &Persist) -> (u64, u64) {
    let persist_bytes = serde_json::to_vec(p).map(|v| v.len()).unwrap_or(0) as u64;
    let drafts_bytes = p.drafts.values().map(|v| v.len()).sum::<usize>() as u64;
    (persist_bytes, drafts_bytes)
}

/// 把字节数压成对人友好的形式（512 / 1.2K / 4.7M），固定到 1 位小数。
fn fmt_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1}M", n as f64 / 1024.0 / 1024.0)
    } else if n >= 1024 {
        format!("{:.1}K", n as f64 / 1024.0)
    } else {
        format!("{n}B")
    }
}

/// 启动诊断报告里的小工具，避免误判字节格式化。
#[cfg(test)]
mod diag_tests {
    use super::fmt_bytes;

    #[test]
    fn bytes_under_kb_use_b() {
        assert_eq!(fmt_bytes(0), "0B");
        assert_eq!(fmt_bytes(512), "512B");
        assert_eq!(fmt_bytes(1023), "1023B");
    }

    #[test]
    fn bytes_kb_use_k() {
        assert_eq!(fmt_bytes(1024), "1.0K");
        assert_eq!(fmt_bytes(1536), "1.5K");
        assert_eq!(fmt_bytes(1024 * 1024 - 1), "1024.0K");
    }

    #[test]
    fn bytes_mb_use_m() {
        let n = 5 * 1024 * 1024 + 512 * 1024; // 5.5MB
        assert_eq!(fmt_bytes(n), "5.5M");
    }
}

#[cfg(test)]
mod dialog_tests {
    use super::{Color32, SettingsRaise, Theme, RAISE_WAIT_FRAMES};
    use crate::RunUiExt;

    /// 软渲染清晰度自适应的契约：剥掉**两个样式槽**（深/浅）里的全部阴影。
    /// 阴影是软件光栅化下「一片糊」的主要来源；只剥当前槽的话，
    /// 用户切一次深浅色就会把灰雾切回来。
    #[test]
    fn soft_render_strips_shadows_in_both_theme_slots() {
        let ctx = egui::Context::default();
        Theme::light().apply(&ctx);
        // 前提：硬件路径下主题确实带阴影（卡片/弹层的柔和感来自这里）
        assert_ne!(
            ctx.style_of(egui::Theme::Light).visuals.popup_shadow,
            egui::epaint::Shadow::NONE,
            "主题不再定义弹层阴影，本测试的前提失效"
        );
        super::FerricApp::apply_soft_render_compat(&ctx);
        for slot in [egui::Theme::Light, egui::Theme::Dark] {
            let v = &ctx.style_of(slot).visuals;
            assert_eq!(v.popup_shadow, egui::epaint::Shadow::NONE, "{slot:?} 弹层");
            assert_eq!(v.window_shadow, egui::epaint::Shadow::NONE, "{slot:?} 卡片");
        }
    }

    /// 软渲染必须关掉**抗锯齿羽化**：它给每个矢量形状的边缘补一圈半透明过渡三角形，
    /// 三角形翻倍 + 逐像素 alpha 混合，正是 CPU 光栅化每帧最大的一笔开销。
    /// 这是「无 GPU 加速也要默认能平稳跑」的关键一环；文字走字体图集，不受影响。
    #[test]
    fn soft_render_disables_feathering() {
        let ctx = egui::Context::default();
        assert!(
            ctx.options(|o| o.tessellation_options.feathering),
            "前提：egui 默认开启羽化，本测试才有意义"
        );
        super::FerricApp::apply_soft_render_compat(&ctx);
        assert!(
            !ctx.options(|o| o.tessellation_options.feathering),
            "软渲染没关羽化 —— 矢量边缘的抗锯齿在 CPU 上每帧都很贵"
        );
    }

    /// 跑 `n` 帧状态机（按「本平台需要重建兜底」跑，即 Wayland 那条路径），
    /// 返回其中要求「跳过渲染」（= 销毁窗口重建）的帧数。
    fn run(r: &mut SettingsRaise, n: usize) -> usize {
        (0..n).filter(|_| r.tick(true)).count()
    }

    /// 同上，但模拟 `Focus` 有效的平台（X11 / Windows / macOS）。
    fn run_no_recreate(r: &mut SettingsRaise, n: usize) -> usize {
        (0..n).filter(|_| r.tick(false)).count()
    }

    /// 窗口已经在前台时再点「设置」：什么都不该发生。
    /// 重建一个本来就看得见的窗口只会白闪一下。
    #[test]
    fn raising_a_focused_window_does_nothing() {
        let mut r = SettingsRaise {
            focused: true,
            ..Default::default()
        };
        r.request();
        assert!(!r.busy(), "已在前台却仍在处理置顶请求");
        assert_eq!(run(&mut r, 30), 0, "已在前台却重建了窗口");
    }

    /// `Focus` 生效（窗口拿到焦点）时，不该再走重建兜底。
    /// 这是 X11 / Windows / macOS 上的正常路径 —— 不能让它们也跟着闪。
    #[test]
    fn focus_arriving_in_time_cancels_the_recreate() {
        let mut r = SettingsRaise::default();
        r.request();
        assert_eq!(run(&mut r, 3), 0, "还在等窗口管理器就急着重建");
        r.focused = true; // 窗口管理器响应了
        assert_eq!(run(&mut r, 30), 0, "Focus 已生效却仍然重建了窗口");
        assert!(!r.busy());
    }

    /// Wayland 上 `Focus` 是空操作，窗口永远拿不到焦点 ——
    /// 等超时后必须回落到「销毁再重建」，否则用户点「设置」毫无反应。
    #[test]
    fn raise_falls_back_to_recreating_the_window() {
        let mut r = SettingsRaise::default();
        r.request();
        // 等待期内先不动窗口，给窗口管理器留出响应时间
        assert_eq!(run(&mut r, RAISE_WAIT_FRAMES as usize - 1), 0);
        // 超时后跳过若干帧不渲染 —— 这几帧里系统窗口被销毁
        let skipped = run(&mut r, 10);
        assert!(skipped > 0, "Focus 没生效却也不重建，点「设置」等于没反应");
        // 兜底是一次性的：重建完就该安静下来，不能一直闪
        assert!(!r.busy(), "重建之后状态没归零，会反复重建");
        assert_eq!(run(&mut r, 30), 0, "重建之后还在继续跳帧");
    }

    /// 没人请求置顶时，状态机不该干扰正常渲染。
    #[test]
    fn idle_raise_state_never_skips_a_frame() {
        let mut r = SettingsRaise::default();
        assert!(!r.busy());
        assert_eq!(run(&mut r, 60), 0);
    }

    /// `Focus` 有效的平台（X11 / Windows / macOS）上**绝不能**走重建兜底。
    ///
    /// 那些平台上重建窗口只是白闪一下；Windows 的 `SetForegroundWindow` 还有自己的
    /// 节流规则，偶尔慢一拍就会被「等 8 帧没焦点」误判成失败 ——
    /// 于是每点一次「设置」就闪一次，正是用户报的「屏闪」之一。
    #[test]
    fn platforms_with_working_focus_never_recreate() {
        let mut r = SettingsRaise::default();
        r.request();
        assert_eq!(
            run_no_recreate(&mut r, 60),
            0,
            "Focus 可用的平台上仍然重建了窗口（会闪）"
        );
        assert!(!r.busy(), "等超时之后状态没归零，会一直请求出帧");
    }

    /// 平台判定本身：只有 Wayland 需要重建兜底。
    #[test]
    fn only_wayland_needs_the_recreate_fallback() {
        let is_wayland = cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some();
        assert_eq!(super::recreate_is_the_only_raise(), is_wayland);
        if !cfg!(target_os = "linux") {
            assert!(
                !super::recreate_is_the_only_raise(),
                "非 Linux 平台不该走重建兜底"
            );
        }
    }

    /// 点「设置」的接线：已经开着才请求置顶（新开的窗口本来就在最前，不必折腾）。
    #[test]
    fn clicking_settings_requests_a_raise_only_when_already_open() {
        let src = include_str!("app.rs");
        assert!(
            src.contains("self.settings_raise = self.settings_open;"),
            "点「设置」时没有按「是否已开着」来决定要不要置顶"
        );
        let body = src
            .split("fn settings_ui(")
            .nth(1)
            .expect("settings_ui 不见了");
        let body = &body[..body.find("fn settings_body(").unwrap_or(body.len())];
        assert!(
            body.contains("ViewportCommand::Focus"),
            "设置窗没有发置顶（Focus）指令"
        );
        assert!(
            body.contains("self.raise.tick("),
            "settings_ui 没有接上置顶状态机，重建兜底不会发生"
        );
    }

    /// 边框必须**真的画出来**，而且要贴着窗口自己的轮廓。
    ///
    /// 设置窗与主界面底色相同，这条边框是两者叠在一起时唯一的边界线索；
    /// 而它取的是 `ctx.content_rect()` —— 画到别的矩形上（比如错用了父窗尺寸）
    /// 就等于没有边框。这里连「有没有」「是不是一条」「在不在正确位置」一起断言：
    /// 单条不透明 1px 是刻意的产品决定 —— 多圈半透明渐变在软件光栅化下糊成脏边。
    #[test]
    fn window_border_hugs_the_window_rect() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(428.0, 620.0));
        let mut strokes = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui_cleared(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    super::draw_window_border(ui.ctx(), false, "test-border");
                },
            );
            strokes = out
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Rect(r) if r.stroke.width > 0.0 => {
                        Some((r.rect, r.stroke.width, r.stroke.color))
                    }
                    _ => None,
                })
                .collect();
        }
        // 只此一条：1px、不透明、贴边（shrink 0.5 半像素对齐）。
        assert_eq!(strokes.len(), 1, "边框应当只有一条描边：{strokes:?}");
        let (r, w, c) = strokes[0];
        assert!((w - 1.0).abs() < 0.01, "边框不是 1px：{w}");
        assert_eq!(c.a(), 255, "边框必须不透明 —— 半透明在软渲染下会糊");
        assert!(
            (r.left() - screen.left()).abs() <= 1.0
                && (screen.right() - r.right()).abs() <= 1.0
                && (r.top() - screen.top()).abs() <= 1.0
                && (screen.bottom() - r.bottom()).abs() <= 1.0,
            "边框没贴住窗口轮廓：{r:?} vs {screen:?}"
        );
    }

    /// 边框颜色与窗口底色必须有**足以看见**的反差。
    ///
    /// 光有描边不够 —— 若颜色和底色几乎一样（此前滚动条就吃过这个亏：白底画
    /// (252,252,253) 的条，等于没画），用户照样看不出两个窗口的边界在哪。
    /// 描边已是不透明色，直接量化色差即可。
    #[test]
    fn window_border_is_visibly_different_from_background() {
        for dark in [false, true] {
            let theme = Theme::from_dark(dark);
            let bg = theme.bg;
            // 与 draw_window_border 同一套取色
            let c = if dark {
                Color32::from_rgb(150, 150, 155)
            } else {
                Color32::from_rgb(60, 60, 66)
            };
            let diff = (c.r() as i32 - bg.r() as i32).abs()
                + (c.g() as i32 - bg.g() as i32).abs()
                + (c.b() as i32 - bg.b() as i32).abs();
            assert!(
                diff > 150,
                "{}主题下边框与底色反差太小（{c:?} vs 底色 {bg:?}，差 {diff}），\
                 两个窗口叠在一起会看不出边界",
                if dark { "暗色" } else { "亮色" }
            );
        }
    }

    /// 设置窗必须画这条边框（否则与主界面完全糊在一起）。
    #[test]
    fn settings_window_draws_the_border() {
        let src = include_str!("app.rs");
        let body = src
            .split("fn settings_ui(")
            .nth(1)
            .expect("settings_ui 不见了");
        let body = &body[..body.find("fn settings_body(").unwrap_or(body.len())];
        assert!(
            body.contains("draw_window_border"),
            "设置窗没画边框 —— 它与主界面底色相同，没边框就看不出边界"
        );
    }

    /// 设置窗在硬件渲染下是**独立的系统窗口**，不是应用内浮层。
    ///
    /// 拖动由窗口管理器负责（自绘标题栏发 `ViewportCommand::StartDrag`），
    /// 所以它能拖到屏幕任何位置 —— 这部分没法在无窗口环境里断言，只能实机验证。
    /// 这里守住的是硬件路径不会退回到「应用内 Window」的老做法。
    ///（软件渲染环境有专门的浮层回退，见下一条测试 —— 那是刻意的，不算退步。）
    #[test]
    fn settings_uses_a_real_os_viewport() {
        let src = include_str!("app.rs");
        let body = src
            .split("fn settings_ui(")
            .nth(1)
            .expect("settings_ui 不见了");
        let body = &body[..body.find("fn settings_body(").unwrap_or(body.len())];
        assert!(
            body.contains("show_viewport_immediate"),
            "设置窗应当开真实系统窗口（show_viewport_immediate）"
        );
        assert!(
            !body.contains("egui::Window::new"),
            "硬件路径的设置窗不该退回成应用内浮层 —— 那样只能在主窗范围里挪，拖不出去"
        );
        assert!(
            body.contains("StartDrag") || src.contains("fn settings_window_chrome"),
            "无边框窗口需要自绘标题栏并发 StartDrag，否则拖不动"
        );
    }

    /// 浮层回退必须**存在且可用**，但**不得按渲染后端自动生效**。
    ///
    /// 曾经是「`gpu_software` 就回退」，理由是软件适配器建不出第二条 wgpu surface。
    /// 实测推翻了这条规则：ARM 虚拟机 + WARP 上第二个视口建得出、画得全、跑得稳，
    /// 用户却永远拿不到与主界面同级的设置窗。所以判据改为**只有显式要求才回退**
    /// （`FERRIC_EMBEDDED_SETTINGS=1`），默认一律真实系统窗口。
    /// 这条测试守的就是「别再退回按环境类别一刀切」。
    #[test]
    fn embedded_settings_is_opt_in_not_automatic() {
        let src = include_str!("app.rs");
        let gate = src
            .split("fn settings_ui(")
            .nth(1)
            .expect("settings_ui 不见了");
        let gate = &gate[..gate.find("fn settings_body(").unwrap_or(gate.len())];
        assert!(
            gate.contains("FERRIC_EMBEDDED_SETTINGS") && gate.contains("settings_embedded_ui"),
            "浮层回退的入口不见了 —— 它得留着，给真正建不出第二条 surface 的环境用"
        );
        // 只看代码，不看注释：这段注释里正要解释「为什么不再按 gpu_software 分流」，
        // 连注释一起扫会把这条说明本身判成违规。
        let code: String = gate
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("gpu_software"),
            "设置窗又按渲染后端一刀切了：软件渲染 ≠ 开不了第二条 surface，\
             实测 WARP 上完全正常，这样判会让整类环境永远拿不到独立窗口"
        );
        // 回退形态必须真的存在，且用应用内 Window（不开新 surface）、
        // 复用同一份 settings_body（功能不缺项）
        let embedded = src
            .split("fn settings_embedded_ui(")
            .nth(1)
            .expect("settings_embedded_ui 不见了");
        let embedded = &embedded[..embedded.find("\n    fn ").unwrap_or(embedded.len())];
        assert!(
            embedded.contains("egui::Window::new"),
            "回退形态应当是应用内 egui::Window"
        );
        assert!(
            embedded.contains("settings_body"),
            "回退形态必须复用 settings_body，否则两种形态功能会漂移"
        );
    }

    /// 提示条是**静止**的，不该把界面钉在满帧率上。
    ///
    /// 曾经的做法是「`frames_left` 每帧减一 + 每帧 `request_repaint()`」：提示在的
    /// 2 秒里整窗按刷新率重画一百多次。更糟的是计时单位是帧 —— 机器越慢，风暴拖得
    /// 越久：软件光栅化（Windows 的 WARP）跑 8fps 时，同样 120 帧要烧 15 秒。
    /// 这正是「Windows 一打开就很卡、Linux 没事」的直接来源（实机归因：启动后
    /// 前 5 秒的重绘全部指向 toasts_ui 与更新下载）。
    #[test]
    fn a_toast_does_not_pin_the_ui_at_full_frame_rate() {
        let src = include_str!("app.rs");
        let body = src.split("fn toasts_ui(").nth(1).expect("toasts_ui 不见了");
        let body = &body[..body.find("\n    fn ").unwrap_or(body.len())];
        assert!(
            !body.contains("request_repaint()"),
            "toasts_ui 里出现了每帧重绘 —— 提示条不动，只需在到期那一刻醒一次"
        );
        assert!(
            body.contains("request_repaint_after"),
            "到期得有人来擦掉它：必须约一个醒点，否则空闲时提示会一直留在屏幕上"
        );
        assert!(
            !body.contains("frames_left"),
            "提示条的寿命必须按时间算 —— 按帧数算会让慢机器上的风暴更久"
        );
    }

    /// 提示条按时间到期，与渲染快慢无关。
    #[test]
    fn toast_expiry_is_measured_in_time() {
        use crate::tool::{Shared, TOAST_TTL};
        let mut shared = Shared::new(Theme::dark());
        shared.toast("测试");
        let t = shared.toasts.last().expect("提示没进队列");
        let left = t.until - std::time::Instant::now();
        assert!(
            left <= TOAST_TTL && left > TOAST_TTL / 2,
            "剩余时长 {left:?} 不在 TOAST_TTL({TOAST_TTL:?}) 的量级上"
        );
        // 到期判定就是「拿当前时刻比一下」，不涉及渲染了多少帧
        assert!(t.until > std::time::Instant::now());
    }

    /// 重启**必须先存盘再拉起新进程**，顺序反了就丢本次会话的草稿。
    ///
    /// 新进程一起来就去读 eframe 的状态文件，而本次改过的草稿要等关窗走
    /// `App::save` 才写下去 —— 先 spawn 后存盘 = 两个进程抢同一份文件，
    /// 新实例读到旧草稿，用户看到「点了重启，刚编辑的内容没了」。
    #[test]
    fn restarting_saves_before_spawning() {
        let src = include_str!("app.rs");
        let body = src
            .split("fn do_restart(")
            .nth(1)
            .expect("do_restart 不见了");
        let body = &body[..body.find("\n    fn ").unwrap_or(body.len())];
        let save_at = body.find("App::save").expect("重启前没有存盘");
        let spawn_at = body.find("relaunch").expect("没有拉起新进程");
        assert!(
            save_at < spawn_at,
            "先拉起新进程再存盘 —— 新实例会读到旧草稿"
        );
        assert!(
            body.contains("flush"),
            "存了但没 flush，落不落盘要看实现心情"
        );
        let close_at = body.find("ViewportCommand::Close").expect("没有关掉自己");
        assert!(spawn_at < close_at, "先关窗就没机会拉起新进程了");
    }

    /// 存盘与关窗都不能在绘制中途做 —— 按钮只置位，由外壳在帧末执行。
    #[test]
    fn the_restart_button_only_sets_a_flag() {
        let src = include_str!("app.rs");
        let body = src
            .split("fn restart_now(")
            .nth(1)
            .expect("restart_now 不见了");
        let body = &body[..body.find("\n    fn ").unwrap_or(body.len())];
        assert!(
            body.contains("want_restart = true"),
            "restart_now 应当只置位"
        );
        assert!(
            !body.contains("relaunch") && !body.contains("ViewportCommand::Close"),
            "在绘制中途关窗/换进程 = 在半张画面上拔插头"
        );
    }

    /// 软件渲染环境必须关掉动画。
    ///
    /// egui 的动画（悬停渐变、浮层淡入）在播放期间每帧都请求重绘整窗，而 egui
    /// 没有局部重绘 —— 有 GPU 时白送，靠 CPU 画时正好相反：动画本身画不满帧，
    /// 却把 CPU 占满。实机上关掉之后，启动 45 秒内的总帧数从 32 降到 18。
    #[test]
    fn software_render_disables_animations() {
        let ctx = egui::Context::default();
        Theme::dark().apply(&ctx);
        assert!(
            ctx.style_of(egui::Theme::Dark).animation_time > 0.0,
            "前提：硬件路径下是有动画的"
        );
        super::FerricApp::apply_soft_render_compat(&ctx);
        for slot in [egui::Theme::Dark, egui::Theme::Light] {
            assert_eq!(
                ctx.style_of(slot).animation_time,
                0.0,
                "{slot:?} 槽里的动画没关 —— 用户切一次深浅色就会把重绘风暴切回来"
            );
        }
    }
}

#[cfg(test)]
mod perf_tests {
    use crate::RunUiExt;
    use std::time::{Duration, Instant};

    /// 跑 `frames` 帧，返回**最快一帧**的耗时。
    ///
    /// 取最快而非最坏：共享 CI runner 上一次调度抖动就能把最坏值推到秒级，
    /// 而最快一帧最贴近代码本身的开销；真有数量级劣化时连它也会被抬起来。
    fn fastest_frame(
        ctx: &egui::Context,
        screen: egui::Rect,
        mut body: impl FnMut(&mut egui::Ui),
    ) -> Duration {
        let mut best = Duration::MAX;
        for i in 0..12 {
            let t0 = Instant::now();
            let _ = ctx.run_ui_cleared(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    ..Default::default()
                },
                &mut body,
            );
            if i >= 4 {
                // 前几帧是预热：字体装载、布局缓存
                best = best.min(t0.elapsed());
            }
        }
        best
    }

    /// 设置窗**内容本身**的构建耗时要足够低。
    ///
    /// 「窗口切换卡」实测下来是 debug 构建 + 无 GPU（软件光栅化）导致的：
    /// release 下整帧 3.0ms、debug 下 18ms，而其中我们自己的 UI 代码只占 3.5ms(debug)
    /// / 0.17ms(release)，其余是多开一个窗口的渲染开销。
    /// 这条守住的是「别让设置面板本身变重」—— 渲染开销改不了，UI 代码的开销能守住。
    ///
    /// # 为什么是**相对**基准，不是绝对毫秒数
    ///
    /// 这条曾两次在 macOS CI 上假失败：先是用「最坏一帧」被调度抖动打死，改成
    /// 「最快一帧 < 16ms」后又炸了一次。根子在于**绝对毫秒数在共享 runner 上
    /// 本质就是 flaky 的** —— 同一份代码，机器忙不忙决定成败，与「面板有没有变重」
    /// 毫无关系。
    ///
    /// 改为在同一轮里量一个**参照工作量**（同样的窗口外壳 + 一个标签），用它吸收
    /// runner 的快慢，再断言真实面板不超过它的若干倍。runner 慢一倍，两个数一起
    /// 慢一倍，比值不动；而面板真变重时比值立刻变大。
    /// 仍留一个绝对下限兜底：参照本身可能快到接近计时精度，比值会失真。
    #[test]
    fn settings_body_is_cheap_to_build() {
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let theme = crate::theme::Theme::light();
        theme.apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(428.0, 620.0));

        // 参照：同样的窗口外壳，内容只有一个标签
        let reference = fastest_frame(&ctx, screen, |ui| {
            let mut open = true;
            super::settings_window_chrome(ui, &theme, &mut open);
            super::draw_window_border(ui.ctx(), false, "perf-ref-border");
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("参照");
            });
        });

        // 实测目标：与设置窗同样的构成（标题栏 + 边框 + 一屏控件）
        let actual = fastest_frame(&ctx, screen, |ui| {
            let mut open = true;
            super::settings_window_chrome(ui, &theme, &mut open);
            super::draw_window_border(ui.ctx(), false, "perf-border");
            egui::CentralPanel::default().show(ui, |ui| {
                for _ in 0..8 {
                    ui.horizontal(|ui| {
                        crate::widgets::field_label(ui, &theme, "示例项");
                        let _ = crate::widgets::seg(ui, &theme, &["甲", "乙", "丙"], 1);
                    });
                    ui.separator();
                }
            });
        });

        // 8 倍 + 16ms 下限：本机 debug 实测比值在 2~3 倍，留足机器差异的余量，
        // 但挡得住「一屏控件突然贵一个数量级」这种真回归。
        let budget = (reference * 8).max(Duration::from_millis(16));
        assert!(
            actual < budget,
            "设置窗内容构建太慢：{actual:?}，预算 {budget:?}（参照一帧 {reference:?}）"
        );
    }
}
