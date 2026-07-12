//! 设计令牌（亮 / 暗），精确对齐 Ferric 最新原型（index.html）色板。

use egui::{Color32, Rounding, Stroke, Visuals};

fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub canvas: Color32,   // 窗口外的桌面底色
    pub bg: Color32,       // 主内容底
    pub rail: Color32,     // 侧栏底
    pub titlebar: Color32, // 标题栏底
    pub fg: Color32,       // 主文字
    pub fg_soft: Color32,  // 次强文字（标题栏名/按钮）
    pub muted: Color32,    // 次要文字
    pub faint: Color32,    // 更弱文字（分组标签/占位）
    pub border: Color32,   // 常规分隔线
    pub border_2: Color32, // 较强边框（卡片/按钮）
    pub field: Color32,    // 输入框底
    pub field_bd: Color32, // 输入框边
    pub code_bg: Color32,  // 代码块 / 搜索框底
    pub accent: Color32,       // 主色（绿）
    pub accent_strong: Color32, // hover / active 主色
    pub accent_soft: Color32,   // 主色浅底（active 导航 / pill）
    pub accent_ring: Color32,   // 聚焦光环
    pub ok: Color32,
    pub danger: Color32,
    pub add_bg: Color32,   // diff 新增行底
    pub add_mark: Color32, // diff 新增字符标记
    pub del_bg: Color32,   // diff 删除行底
    pub del_mark: Color32, // diff 删除字符标记
}

impl Theme {
    pub fn light() -> Self {
        Self {
            dark: false,
            canvas: rgb(0xec, 0xee, 0xf1),
            bg: rgb(0xff, 0xff, 0xff),
            rail: rgb(0xff, 0xff, 0xff),
            titlebar: rgb(0xff, 0xff, 0xff),
            fg: rgb(0x20, 0x27, 0x2e),
            fg_soft: rgb(0x3d, 0x46, 0x50),
            muted: rgb(0x6b, 0x74, 0x7d),
            faint: rgb(0x9a, 0xa2, 0xab),
            border: rgb(0xeb, 0xed, 0xf0),
            border_2: rgb(0xe0, 0xe3, 0xe7),
            field: rgb(0xff, 0xff, 0xff),
            field_bd: rgb(0xd8, 0xdc, 0xe1),
            code_bg: rgb(0xf7, 0xf8, 0xfa),
            accent: rgb(0x18, 0xa0, 0x58),
            accent_strong: rgb(0x14, 0x89, 0x4b),
            accent_soft: rgb(0xe9, 0xf6, 0xee),
            accent_ring: rgba(0x18, 0xa0, 0x58, 46), // 0.18
            ok: rgb(0x18, 0xa0, 0x58),
            danger: rgb(0xd9, 0x53, 0x4f),
            add_bg: rgb(0xe7, 0xf6, 0xec),
            add_mark: rgb(0xb8, 0xe6, 0xc6),
            del_bg: rgb(0xfd, 0xec, 0xec),
            del_mark: rgb(0xf6, 0xc9, 0xc7),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            canvas: rgb(0x0e, 0x11, 0x16),
            bg: rgb(0x18, 0x1c, 0x22),
            rail: rgb(0x1b, 0x20, 0x27),
            titlebar: rgb(0x1b, 0x20, 0x27),
            fg: rgb(0xee, 0xf1, 0xf4),
            fg_soft: rgb(0xcb, 0xd2, 0xd9),
            muted: rgb(0x8b, 0x94, 0x9e),
            faint: rgb(0x62, 0x6c, 0x76),
            border: rgba(0xff, 0xff, 0xff, 18),   // 0.07
            border_2: rgba(0xff, 0xff, 0xff, 28),  // 0.11
            field: rgb(0x12, 0x16, 0x1b),
            field_bd: rgba(0xff, 0xff, 0xff, 31),  // 0.12
            code_bg: rgb(0x12, 0x16, 0x1b),
            accent: rgb(0x2b, 0xb5, 0x6a),
            accent_strong: rgb(0x39, 0xc1, 0x76),
            accent_soft: rgba(0x2b, 0xb5, 0x6a, 38), // 0.15
            accent_ring: rgba(0x2b, 0xb5, 0x6a, 64), // 0.25
            ok: rgb(0x2b, 0xb5, 0x6a),
            danger: rgb(0xe0, 0x6b, 0x67),
            add_bg: rgba(0x2b, 0xb5, 0x6a, 33),   // 0.13
            add_mark: rgba(0x2b, 0xb5, 0x6a, 84),  // 0.33
            del_bg: rgba(0xe0, 0x6b, 0x67, 36),   // 0.14
            del_mark: rgba(0xe0, 0x6b, 0x67, 84),  // 0.33
        }
    }

    pub fn from_dark(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// 把主题映射到 egui 全局 Visuals，让内置控件也跟随配色。
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.override_text_color = Some(self.fg);
        v.panel_fill = self.bg;
        v.window_fill = self.bg;
        v.extreme_bg_color = self.code_bg; // TextEdit 底：用浅灰 code_bg，避免白底叠白页发平
        v.faint_bg_color = self.code_bg;
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, self.accent);

        // 去掉控件的自动描边（按钮/输入/下拉不再有硬边），靠底色区分。
        v.widgets.noninteractive.bg_stroke = Stroke::NONE;
        v.widgets.inactive.bg_fill = self.code_bg;
        v.widgets.inactive.weak_bg_fill = self.code_bg;
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.hovered.bg_fill = self.border;
        v.widgets.hovered.weak_bg_fill = self.border;
        v.widgets.hovered.bg_stroke = Stroke::NONE;
        v.widgets.active.bg_fill = self.accent_soft;
        v.widgets.active.weak_bg_fill = self.accent_soft;
        v.widgets.active.bg_stroke = Stroke::new(1.5, self.accent); // 仅聚焦时显示主色环
        v.widgets.open.bg_stroke = Stroke::NONE;
        v.widgets.open.bg_fill = self.code_bg;

        // 统一控件圆角（输入框 / 下拉 / 按钮）。
        let r = Rounding::same(8.0);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = r;
            w.expansion = 0.0;
        }
        v.menu_rounding = Rounding::same(10.0);
        v.window_rounding = Rounding::same(12.0);
        v.window_stroke = Stroke::new(1.0, self.border_2);
        v.window_fill = self.bg;
        v.popup_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 6.0),
            blur: 18.0,
            spread: 0.0,
            color: Color32::from_black_alpha(40),
        };
        ctx.set_visuals(v);

        // 更舒展的间距。
        ctx.style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(10.0, 6.0);
            s.spacing.interact_size.y = 30.0;
        });
    }
}
