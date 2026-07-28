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
        }
    }
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
}

impl FerricApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        fonts::install_fonts(&cc.egui_ctx);

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

        // 清掉上次遗留的更新暂存目录 —— 留在盘上的旧安装包本身就是个可被替换的靶子
        crate::updater::cleanup_stale();

        let mut tools = views::registry();
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

        let mut shared = Shared::new(theme);
        shared.lang = persist.lang;
        for w in plugin_warns {
            shared.toast(format!("插件加载失败 · {w}"));
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
        }
    }

    /// 当前生效的更新服务器。None = 本构建未配置（更新功能整体禁用，
    /// **绝不回落到「去问服务端要公钥」**，那等于让对方自报家门）。
    fn server_profile(&self) -> Option<crate::net::ServerProfile> {
        self.server_override
            .clone()
            .or_else(crate::net::ServerProfile::builtin)
    }

    /// 是不是编译期烘入的那个服务器。不是的话自动安装会被禁用。
    fn server_is_builtin(&self) -> bool {
        self.server_override
            .as_ref()
            .map_or(true, |p| p.is_builtin())
    }

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
        }
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
                            env!("CARGO_PKG_VERSION"),
                            " (build ",
                            env!("FERRIC_BUILD_NUMBER"),
                            ") · 本地开发者工具箱"
                        ));
                    }
                    if widgets::icon_btn(ui, &theme, icons::SETTINGS, 18.0).clicked() {
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
                RichText::new(concat!(
                    "v",
                    env!("CARGO_PKG_VERSION"),
                    ".",
                    env!("FERRIC_BUILD_NUMBER"),
                    " · rust"
                ))
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

    // ---------- 窗口发光边框 ----------

    /// 沿窗口边缘画一圈深灰半透明发散边框（内发光），全局常显：
    /// 无边框方角窗口下与轮廓完全贴合，用于和桌面上其他窗口区分。
    fn window_glow_ui(&self, ctx: &egui::Context) {
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("window-glow"),
        ));
        // 亮色主题用深灰，暗色主题提亮一档保证可见。
        let c = if self.shared.theme.dark {
            Color32::from_rgb(150, 150, 155)
        } else {
            Color32::from_rgb(60, 60, 66)
        };
        let fade = |a: u8| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
        // 内发光：由外向内逐圈变透明（刻意收着，避免喧宾夺主）
        for (inset, alpha) in [(2.0_f32, 30_u8), (4.0, 16), (6.0, 7)] {
            painter.rect_stroke(
                rect.shrink(inset),
                CornerRadius::ZERO,
                Stroke::new(3.0_f32, fade(alpha)),
                egui::StrokeKind::Inside,
            );
        }
        // 边缘细线（半透明，不抢内容）
        painter.rect_stroke(
            rect.shrink(0.5),
            CornerRadius::ZERO,
            Stroke::new(1.5_f32, fade(150)),
            egui::StrokeKind::Inside,
        );
    }

    // ---------- 设置弹窗 ----------

    fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let theme = self.shared.theme;
        let mut open = self.settings_open;
        egui::Window::new(RichText::new("设置").size(15.0).color(theme.fg))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(theme.bg)
                    .stroke(Stroke::new(1.0_f32, theme.border_2))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::same(18)),
            )
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_width(384.0);
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
                self.update_settings_ui(ui);
                ui.add_space(10.0);
                ui.label(
                    RichText::new(concat!(
                        "Ferric v",
                        env!("CARGO_PKG_VERSION"),
                        ".",
                        env!("FERRIC_BUILD_NUMBER"),
                        // 接入自动更新后，「不上传」不再成立：检查更新会把本机版本号
                        // 发给更新服务器。文案要如实，不能留一句已经不真的宣称。
                        " · 工具数据仅存于本机；仅检查更新时联网"
                    ))
                    .family(FontFamily::Monospace)
                    .size(11.0)
                    .color(theme.faint),
                );
            });
        self.settings_open = open;
    }

    /// 设置弹窗里的「软件更新」区块。
    ///
    /// 这里刻意把**服务器身份**摊开给用户看（地址 + 公钥指纹），因为自定义服务器的
    /// 配置存在用户可写的 ron 文件里、能被同用户的恶意进程静默改写 —— 可见性是用户
    /// 发现这件事的唯一途径。同理，非内置服务器时自动安装会被禁用。
    fn update_settings_ui(&mut self, ui: &mut egui::Ui) {
        use crate::updater::Phase;
        let theme = self.shared.theme;

        let Some(profile) = self.server_profile() else {
            widgets::field_label(ui, &theme, "软件更新");
            ui.label(
                RichText::new("本构建未配置更新服务器，自动更新不可用")
                    .size(11.5)
                    .color(theme.faint),
            );
            return;
        };
        let builtin = self.server_is_builtin();

        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "软件更新");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let busy = self.updater.busy();
                ui.add_enabled_ui(!busy, |ui| {
                    if widgets::ghost_button(ui, &theme, "检查更新").clicked() {
                        let ctx = ui.ctx().clone();
                        self.updater.check(profile.clone(), &ctx);
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

        // 服务器身份：地址 + 公钥指纹。指纹便于口头核对。
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
        if !builtin {
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
                let (url, key) = self
                    .server_draft
                    .get_or_insert_with(|| (profile.base_url.clone(), profile.pubkey.clone()));
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
                    if builtin {
                        if widgets::ghost_button(ui, &theme, "下载并校验").clicked() {
                            let ctx = ui.ctx().clone();
                            self.updater.download(profile.clone(), info.clone(), &ctx);
                        }
                    } else {
                        // 自定义服务器：只通知，不下载不执行
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
                let hint = match std::env::consts::OS {
                    "windows" => "将启动安装程序并退出 Ferric",
                    "macos" => "将打开安装包，需你手动完成安装",
                    _ => "将交给系统的软件安装器，可能需要授权",
                };
                ui.label(RichText::new(hint).size(11.0).color(theme.faint));
                if widgets::ghost_button(ui, &theme, "立即安装").clicked() {
                    match crate::updater::launch(&file) {
                        Ok(()) => {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => {
                            self.updater.phase = Phase::Failed(e);
                        }
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
        };
        eframe::set_value(storage, eframe::APP_KEY, &persist);
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        let ctx = &ctx;
        self.debug_screenshot(ctx);
        // 跟随系统模式下与操作系统深浅色保持同步（含启动首帧与运行中切换）。
        self.sync_theme(ctx);

        // 更新进度轮询必须在**外壳顶层**，不能挂在某个视图里 ——
        // 更新是全局的，挂在视图里会导致用户不切到那页就永远收不到结果。
        self.updater.poll(ctx);
        // 插件市场视图需要知道当前服务器；每帧同步，覆盖设置后立刻生效
        self.shared.server = self.server_profile();

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

        // 全局窗口发散边框
        self.window_glow_ui(ctx);

        self.settings_ui(ctx);
        self.toasts_ui(ctx);
    }
}
