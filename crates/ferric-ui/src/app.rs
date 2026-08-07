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
    code_font: crate::widgets::code_editor::FontCfg,
    /// 自动检查更新并在后台下载。默认开 —— 更新只有及时装上才有意义。
    #[serde(default = "default_true")]
    auto_update: bool,
    /// 是否使用演示数据：`None` = 自动（没烘入服务器时才用），`Some` = 用户显式选择。
    #[serde(default)]
    mock_source: Option<bool>,
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
    /// 演示数据开关：None = 自动。
    mock_source: Option<bool>,
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
    /// 本次运行实际拿到的图形适配器描述（「后端 · 显卡名」），设置页展示。
    /// None = 非 wgpu 渲染路径（理论上不会发生，防御性处理）。
    gpu_desc: Option<String>,
    /// 实际适配器是 CPU 软件光栅化（WARP / llvmpipe）—— 虚拟机与无驱动环境。
    /// 设置页据此提示「画面糊 / 卡的根源在这，换后端或开 3D 加速」。
    gpu_software: bool,
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
        let has_cjk = fonts::install_fonts(&cc.egui_ctx);

        let persist: Persist = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();

        // 迁移：旧数据没有 theme_mode，一律改为跟随系统（旧版的 dark 只作首帧兜底）。
        let mode = persist.theme_mode.unwrap_or(ThemeMode::System);
        // 启动首帧系统主题可能尚未上报（system_theme() 为 None），
        // 先用上次生效的深浅色兜底，进入 update() 后每帧与系统同步。
        let dark = match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => cc
                .egui_ctx
                .system_theme()
                .map_or(persist.dark, |t| t == egui::Theme::Dark),
        };
        let theme = Theme::from_dark(dark);
        theme.apply(&cc.egui_ctx);

        // 记下真实拿到的适配器：设置页「渲染后端」区块要展示「现在实际用的是什么」。
        // 用户切后端做 A/B 对比时，没有这行就无从确认切换是否真的生效。
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
                let sw = info.device_type == eframe::wgpu::DeviceType::Cpu;
                (Some(format!("{backend} · {}", info.name)), sw)
            }
            None => (None, false),
        };
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
        cc.egui_ctx.set_zoom_factor(ui_scale);

        let mut shared = Shared::new(theme);
        shared.lang = persist.lang;
        shared.code_font = persist.code_font.clamped();
        shared.gpu_software = gpu_software;
        // 软渲染环境的清晰度自适应要在首帧前生效，否则第一屏就带着灰雾阴影。
        if gpu_software {
            Self::strip_soft_render_haze(&cc.egui_ctx);
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
            mock_source: persist.mock_source,
            last_update_check: persist.last_update_check,
            builtin_tools,
            was_focused: true,
            last_outer_pos: None,
            last_moved_at: None,
            fast_present: false,
            gpu_desc,
            gpu_software,
        }
    }

    /// 当前生效的数据源（真服务端 or 演示数据）。
    fn source(&self) -> Option<crate::source::Source> {
        crate::source::Source::resolve(self.mock_source, self.server_profile())
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

    // 「是不是内置服务器」现在问数据源本身（`Source::is_builtin_server`）——
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
            // 主题重铺会把阴影写回来 —— 软渲染环境要立刻再剥掉。
            if self.gpu_software {
                Self::strip_soft_render_haze(ctx);
            }
            // 换主题会重铺样式，字号缩放要跟着重新落一次
            self.applied_ui_scale = None;
        }
        self.sync_ui_scale(ctx);
    }

    /// 软件光栅化（WARP / llvmpipe）下的清晰度自适应。
    ///
    /// 阴影是大范围半透明渐变：硬件渲染下是「柔和」，软渲染 + 低分屏下叠在内容
    /// 边上就是一圈灰雾 —— 用户的原话是「一片糊」。这类环境里全部清零，
    /// 界面靠 1px 边框与底色分层；直角、文本的像素对齐 egui 默认已开。
    /// 硬件渲染路径完全不受影响。
    fn strip_soft_render_haze(ctx: &egui::Context) {
        ctx.all_styles_mut(|s| {
            s.visuals.window_shadow = egui::epaint::Shadow::NONE;
            s.visuals.popup_shadow = egui::epaint::Shadow::NONE;
        });
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
                top: 10,
                bottom: 4,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().top(),
                    Stroke::new(1.0_f32, theme.border),
                );
                ui.add_space(10.0);
                // 品牌一行
                ui.horizontal(|ui| {
                    self.brand(ui);
                });
                ui.add_space(10.0);
                // 图标一行（主题 / 关于 / 设置），紧凑左对齐，避免与品牌重叠
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
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
                    // 语言切换（显示当前语言，点击切换 中/EN）
                    let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
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

    fn brand(&self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        // 渐变方块 logo（用 accent 填色近似渐变）
        let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), theme.accent);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icons::BOX,
            FontId::new(18.0, icons::family()),
            Color32::WHITE,
        );
        ui.add_space(9.0);
        ui.vertical(|ui| {
            ui.add_space(1.0);
            ui.label(
                RichText::new("Ferric")
                    .family(FontFamily::Name(UI_BOLD.into()))
                    .size(16.0)
                    .color(theme.fg),
            );
            ui.label(
                RichText::new(concat!("v", env!("FERRIC_VERSION"), " · rust"))
                    .family(FontFamily::Monospace)
                    .size(10.0)
                    .color(theme.faint),
            );
        });
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
        }
    }

    // ---------- 内容区 ----------

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
                .with_inner_size([428.0, 620.0])
                .with_min_inner_size([380.0, 320.0])
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
                    .show(ui, |ui| {
                        ScrollArea::vertical().show(ui, |ui| {
                            self.settings_body(ui);
                        });
                    });
            },
        );
        self.settings_open = open;
        self.raise.focused = focused;
    }

    /// 设置窗的内容（与承载它的窗口形态无关）。
    fn settings_body(&mut self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        {
            let ui = &mut *ui;
            {
                // 外观
                ui.horizontal(|ui| {
                    widgets::field_label(ui, &theme, "外观");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let sel = match self.mode {
                            ThemeMode::System => 0,
                            ThemeMode::Light => 1,
                            ThemeMode::Dark => 2,
                        };
                        if let Some(n) =
                            widgets::seg(ui, &theme, &["跟随系统", "亮色", "暗色"], sel)
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
                ui.add_space(4.0);
                ui.separator();
                self.font_settings_ui(ui);
                ui.separator();
                // 侧边栏宽度
                ui.horizontal(|ui| {
                    widgets::field_label(ui, &theme, "侧边栏宽度");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::ghost_button(ui, &theme, "恢复默认").clicked() {
                            self.rail_width = RAIL_DEFAULT;
                        }
                    });
                });
                ui.separator();
                // 本地数据
                ui.horizontal(|ui| {
                    widgets::field_label(ui, &theme, "本地数据");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::ghost_button(ui, &theme, "清除收藏与工具草稿").clicked()
                        {
                            self.favorites.clear();
                            // 草稿在 save() 时由工具状态重建，重置工具即清除草稿。
                            self.tools = views::registry();
                            self.shared.toast("已清除收藏与全部工具草稿");
                        }
                    });
                });
                ui.separator();
                self.renderer_settings_ui(ui);
                ui.separator();
                self.update_settings_ui(ui);
                ui.add_space(10.0);
                ui.label(
                    RichText::new(concat!(
                        "Ferric v",
                        env!("FERRIC_VERSION"),
                        // 接入自动更新后，「不上传」不再成立：检查更新会把本机版本号
                        // 发给更新服务器。文案要如实，不能留一句已经不真的宣称。
                        " · 工具数据仅存于本机；仅检查更新时联网"
                    ))
                    .family(FontFamily::Monospace)
                    .size(11.0)
                    .color(theme.faint),
                );
            }
        }
    }

    /// 设置弹窗里的「字体」区块：全局界面字号 + 代码编辑区排版。
    ///
    /// 两类字号刻意分开：界面字号影响整个外壳（侧栏、按钮、说明文字），而 JSON 这类
    /// 编辑区常常要单独调大来核对密钥、或单独调小来纵览长文档，把它们绑在一起反而难用。
    /// 代码这三项与 JSON 工具条上的字体菜单是**同一份配置**，从哪边改都一样生效。
    fn font_settings_ui(&mut self, ui: &mut egui::Ui) {
        use crate::widgets::code_editor::FontCfg;
        let theme = self.shared.theme;

        // —— 界面字号
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
                        self.shared.toast(format!(
                            "渲染后端已设为 {}，重启 Ferric 后生效",
                            want.label()
                        ));
                    }
                }
            });
        });
        ui.label(
            RichText::new("画面闪烁 / 撕裂 / 打不开时换一个试试；「自动」由系统挑。重启后生效")
                .size(11.0)
                .color(theme.faint),
        );
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
                const CHOICES: [(Option<bool>, &str); 3] = [
                    (None, "自动"),
                    (Some(false), "服务器"),
                    (Some(true), "演示"),
                ];
                let names: Vec<&str> = CHOICES.iter().rev().map(|(_, n)| *n).collect();
                let nat = CHOICES
                    .iter()
                    .position(|(v, _)| *v == self.mock_source)
                    .unwrap_or(0);
                if let Some(n) = widgets::seg(ui, &theme, &names, CHOICES.len() - 1 - nat) {
                    let want = CHOICES[CHOICES.len() - 1 - n].0;
                    if want != self.mock_source {
                        self.mock_source = want;
                        // 换了源，之前那次检查的结论就作废了
                        self.updater.phase = Phase::Idle;
                        self.shared.toast(match self.source() {
                            Some(s) if s.is_mock() => "已切换到演示数据（不联网）",
                            Some(_) => "已切换到更新服务器",
                            None => "本构建未配置服务器，且演示数据已关闭",
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
        let builtin = source.is_builtin_server();

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
                ui.label(
                    RichText::new(format!("公钥指纹 {}", profile.fingerprint()))
                        .family(FontFamily::Monospace)
                        .size(10.5)
                        .color(if builtin { theme.faint } else { theme.danger }),
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

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let theme = self.shared.theme;
        self.shared.toasts.retain(|t| t.frames_left > 0);
        for t in self.shared.toasts.iter_mut() {
            t.frames_left = t.frames_left.saturating_sub(1);
        }
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
            ctx.request_repaint();
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
    ///    错一帧就是全屏残影。窗口位置一变就临时把表面切到 AutoNoVsync（wgpu 的
    ///    DX12 交换链常开 ALLOW_TEARING，present 不再排队等 vblank），位置稳定
    ///    300ms 后切回 [`eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY`]。
    ///    代价是拖动瞬间可能有轻微撕裂，但远好于整窗残影。
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
        if moving != self.fast_present {
            self.fast_present = moving;
            frame.set_wgpu_surface_config(if moving {
                eframe::egui_wgpu::SurfaceConfig {
                    present_mode: eframe::wgpu::PresentMode::AutoNoVsync,
                    desired_maximum_frame_latency: Some(1),
                }
            } else {
                eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY
            });
        }
        if moving {
            // 移动期间保持出帧：位置一变就有新画面可 present，残影窗口最短。
            ctx.request_repaint();
        }
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
        let drafts = self
            .tools
            .iter()
            .filter_map(|t| t.save_draft().map(|d| (t.meta().id.to_owned(), d)))
            .collect();
        let persist = Persist {
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
            mock_source: self.mock_source,
            last_update_check: self.last_update_check,
        };
        eframe::set_value(storage, eframe::APP_KEY, &persist);
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        let ctx = &ctx;
        self.present_hygiene(ctx, frame);
        // 稳定出帧 = 本次启动成功。把当前渲染后端记成 last_good 并清掉「正在尝试」
        // 标记，否则下次启动会以为上次是崩在启动路上的（见 launch::plan）。
        if self.frames < STABLE_FRAMES {
            self.frames += 1;
            if self.frames == STABLE_FRAMES {
                // 锁定的后端被证明不可用时会被改回「自动」—— 必须让用户知道，
                // 否则设置里显示的选项和实际行为对不上。
                if let Some(b) = crate::launch::mark_running() {
                    self.launch_cfg = crate::launch::load();
                    self.shared.toast(format!(
                        "渲染后端 {} 在本机不可用，已改回「自动」",
                        b.label()
                    ));
                }
            }
        }
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
        if let crate::updater::Tick::ReadyToInstall { version } = tick {
            self.shared.toast(format!(
                "v{version} 已下载并校验通过 · 点右上角「安装」即可更新"
            ));
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

        // 插件热加载放在**所有视图渲染完之后**：市场视图正是 self.tools 里的一员，
        // 在它的 ui() 里改这个向量是不可能的（元素正被借着）。
        if std::mem::take(&mut self.shared.reload_plugins) {
            self.reload_plugins();
        }
    }
}

#[cfg(test)]
mod dialog_tests {
    use super::{Color32, SettingsRaise, Theme, RAISE_WAIT_FRAMES};

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
        super::FerricApp::strip_soft_render_haze(&ctx);
        for slot in [egui::Theme::Light, egui::Theme::Dark] {
            let v = &ctx.style_of(slot).visuals;
            assert_eq!(v.popup_shadow, egui::epaint::Shadow::NONE, "{slot:?} 弹层");
            assert_eq!(v.window_shadow, egui::epaint::Shadow::NONE, "{slot:?} 卡片");
        }
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
            let out = ctx.run_ui(
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

    /// 设置窗是**独立的系统窗口**，不是应用内浮层。
    ///
    /// 拖动由窗口管理器负责（自绘标题栏发 `ViewportCommand::StartDrag`），
    /// 所以它能拖到屏幕任何位置 —— 这部分没法在无窗口环境里断言，只能实机验证。
    /// 这里守住的是不会退回到「应用内 Window」的老做法。
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
            "设置窗不该退回成应用内浮层 —— 那样只能在主窗范围里挪，拖不出去"
        );
        assert!(
            body.contains("StartDrag") || src.contains("fn settings_window_chrome"),
            "无边框窗口需要自绘标题栏并发 StartDrag，否则拖不动"
        );
    }
}

#[cfg(test)]
mod perf_tests {
    /// 设置窗**内容本身**的构建耗时要足够低。
    ///
    /// 「窗口切换卡」实测下来是 debug 构建 + 这台机器无 GPU（软件光栅化）导致的：
    /// release 下整帧 3.0ms、debug 下 18ms，而其中我们自己的 UI 代码只占 3.5ms(debug)
    /// / 0.17ms(release)，其余是多开一个窗口的渲染开销。
    ///
    /// 这条守住的是「别让设置面板本身变重」—— 渲染开销改不了，UI 代码的开销能守住。
    #[test]
    fn settings_body_is_cheap_to_build() {
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let theme = crate::theme::Theme::light();
        theme.apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(428.0, 620.0));

        // 先跑几帧预热（字体装载、布局缓存）
        let mut worst = std::time::Duration::ZERO;
        for i in 0..12 {
            let t0 = std::time::Instant::now();
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    ..Default::default()
                },
                |ui| {
                    // 与设置窗同样的构成：标题栏 + 边框 + 一屏控件
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
                },
            );
            if i >= 4 {
                worst = worst.max(t0.elapsed());
            }
        }
        // debug 构建下也应远低于一帧预算；这里给足余量，只拦住数量级劣化
        assert!(
            worst.as_millis() < 16,
            "设置窗内容构建太慢：最坏 {:?}（debug 构建实测约 3-4ms）",
            worst
        );
    }
}
