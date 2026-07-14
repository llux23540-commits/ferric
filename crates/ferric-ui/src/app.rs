//! 应用外壳：状态、布局、路由、持久化。

use crate::chrome::{self, TITLE_BAR_HEIGHT};
use crate::fonts::{UI_BOLD, UI_SEMIBOLD};
use crate::theme::Theme;
use crate::tool::{Shared, Tool};
use crate::{fonts, icons, views, widgets};
use egui::{
    vec2, Align, Align2, CentralPanel, Color32, FontFamily, FontId, Frame, Key, Layout, Margin,
    RichText, Rounding, ScrollArea, Sense, SidePanel, Stroke, TopBottomPanel,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const APP_NAME: &str = "Ferric";

const RAIL_DEFAULT: f32 = 264.0;
const RAIL_MIN: f32 = 196.0;
const RAIL_MAX: f32 = 460.0;

#[derive(Serialize, Deserialize)]
struct Persist {
    dark: bool,
    rail_width: f32,
    favorites: Vec<String>,
    active_id: String,
    #[serde(default)]
    drafts: HashMap<String, String>,
}

impl Default for Persist {
    fn default() -> Self {
        Self {
            dark: false,
            rail_width: RAIL_DEFAULT,
            favorites: Vec::new(),
            active_id: "json".to_owned(),
            drafts: HashMap::new(),
        }
    }
}

pub struct FerricApp {
    tools: Vec<Box<dyn Tool>>,
    active: usize,
    dark: bool,
    rail_width: f32,
    favorites: HashSet<String>,
    rail_filter: String,
    focus_search: bool,
    settings_open: bool,
    shared: Shared,
}

impl FerricApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        fonts::install_fonts(&cc.egui_ctx);

        let persist: Persist = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();

        let theme = Theme::from_dark(persist.dark);
        theme.apply(&cc.egui_ctx);

        let mut tools = views::registry();
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

        Self {
            tools,
            active,
            dark: persist.dark,
            rail_width: persist.rail_width.clamp(RAIL_MIN, RAIL_MAX),
            favorites: persist.favorites.into_iter().collect(),
            rail_filter: String::new(),
            focus_search: false,
            settings_open: false,
            shared: Shared::new(theme),
        }
    }

    fn set_dark(&mut self, ctx: &egui::Context, dark: bool) {
        self.dark = dark;
        self.shared.theme = Theme::from_dark(dark);
        self.shared.theme.apply(ctx);
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
        Frame::none()
            .fill(theme.code_bg)
            .rounding(Rounding::same(11.0))
            .inner_margin(Margin::symmetric(12.0, 0.0))
            .show(ui, |ui| {
                ui.set_height(40.0);
                ui.horizontal_centered(|ui| {
                    ui.label(icons::text(icons::SEARCH, 16.0, theme.muted));
                    ui.add_space(8.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.rail_filter)
                            .frame(false)
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
        TopBottomPanel::bottom("rail-foot")
            .frame(Frame::none().inner_margin(Margin {
                left: 2.0,
                right: 2.0,
                top: 10.0,
                bottom: 4.0,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| {
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
                    if widgets::icon_btn(ui, &theme, tmoon, 18.0).clicked() {
                        let want = !self.dark;
                        self.set_dark(ui.ctx(), want);
                    }
                    if widgets::icon_btn(ui, &theme, icons::INFO, 18.0).clicked() {
                        self.shared.toast("Ferric v0.4.2 · 本地开发者工具箱");
                    }
                    if widgets::icon_btn(ui, &theme, icons::SETTINGS, 18.0).clicked() {
                        self.settings_open = true;
                    }
                });
            });

        // 导航列表
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);
                let filter = self.rail_filter.to_lowercase();
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
        m.name.to_lowercase().contains(filter)
            || m.keywords.iter().any(|k| k.contains(filter))
    }

    fn brand(&self, ui: &mut egui::Ui) {
        let theme = self.shared.theme;
        // 渐变方块 logo（用 accent 填色近似渐变）
        let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, Rounding::same(9.0), theme.accent);
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
                RichText::new("v0.4.2 · rust")
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
            ui.painter().rect_filled(rect, Rounding::same(9.0), fill);
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
                    ui.painter().rect_filled(rect, Rounding::same(9.0), theme.border);
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
                ui.painter().rect_filled(bar, Rounding::same(3.0), theme.accent);
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
                Frame::none()
                    .fill(theme.bg)
                    .stroke(Stroke::new(1.0_f32, theme.border_2))
                    .rounding(Rounding::same(14.0))
                    .inner_margin(Margin::same(18.0)),
            )
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_width(384.0);
                // 外观
                ui.horizontal(|ui| {
                    widgets::field_label(ui, &theme, "外观");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let sel = if self.dark { 1 } else { 0 };
                        if let Some(n) = widgets::seg(ui, &theme, &["亮色", "暗色"], sel) {
                            let want = n == 1;
                            if want != self.dark {
                                self.set_dark(ui.ctx(), want);
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
                        if widgets::ghost_button(ui, &theme, "清除收藏与工具草稿").clicked() {
                            self.favorites.clear();
                            // 草稿在 save() 时由工具状态重建，重置工具即清除草稿。
                            self.tools = views::registry();
                            self.shared.toast("已清除收藏与全部工具草稿");
                        }
                    });
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new(concat!("Ferric v", env!("CARGO_PKG_VERSION"), " · 全部数据仅存于本机，不上传"))
                        .family(FontFamily::Monospace)
                        .size(11.0)
                        .color(theme.faint),
                );
            });
        self.settings_open = open;
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
                    Frame::none()
                        .fill(theme.fg)
                        .rounding(Rounding::same(10.0))
                        .inner_margin(Margin::symmetric(16.0, 9.0))
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
        "JSON" => icons::CODE,
        "对比" => icons::GIT_COMPARE,
        "转换" => icons::CLOCK,
        "SQL" => icons::DATABASE,
        "生成" => icons::CREDIT_CARD,
        "加密" => icons::LOCK,
        "文本" => icons::TERMINAL,
        _ => icons::BOX,
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
            rail_width: self.rail_width,
            favorites: self.favorites.iter().cloned().collect(),
            active_id: self.tools[self.active].meta().id.to_owned(),
            drafts,
        };
        eframe::set_value(storage, eframe::APP_KEY, &persist);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        let root = Frame::none().fill(theme.bg);

        CentralPanel::default().frame(root).show(ctx, |ui| {
            TopBottomPanel::top("titlebar")
                .exact_height(TITLE_BAR_HEIGHT)
                .frame(Frame::none().fill(theme.titlebar))
                .show_separator_line(false)
                .show_inside(ui, |ui| {
                    chrome::title_bar_content(ui, &theme);
                });

            let rail_resp = SidePanel::left("rail")
                .resizable(true)
                .min_width(RAIL_MIN)
                .max_width(RAIL_MAX)
                .default_width(self.rail_width)
                .frame(
                    Frame::none()
                        .fill(theme.rail)
                        .inner_margin(Margin::symmetric(12.0, 6.0)),
                )
                .show_separator_line(false)
                .show_inside(ui, |ui| {
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
                .frame(Frame::none().fill(theme.bg))
                .show_inside(ui, |ui| {
                    TopBottomPanel::top("topbar")
                        .exact_height(60.0)
                        .frame(Frame::none().fill(theme.bg))
                        .show_separator_line(false)
                        .show_inside(ui, |ui| {
                            self.topbar_ui(ui);
                        });
                    CentralPanel::default()
                        .frame(Frame::none().fill(theme.bg))
                        .show_inside(ui, |ui| {
                            self.content_body(ui);
                        });
                });
        });

        self.settings_ui(ctx);
        self.toasts_ui(ctx);
    }
}
