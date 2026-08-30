//! 编辑器配色与字体配置。

use egui::{Color32, FontFamily};

/// 编辑器用到的全部颜色。从宿主应用的主题里挑出这些字段即可接入，
/// 不必依赖宿主的整套主题类型。
#[derive(Clone, Copy)]
pub struct Colors {
    /// 深色主题（影响数字等默认配色）。
    pub dark: bool,
    /// 编辑器底色（行号栏 / 横向滚动时遮罩正文）。
    pub bg: Color32,
    /// 主文字色。
    pub fg: Color32,
    /// 标点 / 空白 / 弱文本。
    pub muted: Color32,
    /// 更弱的文本（滚动条、占位）。
    pub faint: Color32,
    /// 主色（键名、搜索命中、光标跟随高亮）。
    pub accent: Color32,
    /// 主色的强版本（hover / 字符串键名）。
    pub accent_strong: Color32,
    /// 错误 / true·false 字面量。
    pub danger: Color32,
    /// 成功 / 字符串值。
    pub ok: Color32,
    /// 分隔线。
    pub border: Color32,
}

impl Colors {
    pub fn light() -> Self {
        Self {
            dark: false,
            bg: Color32::from_rgb(0xff, 0xff, 0xff),
            fg: Color32::from_rgb(0x20, 0x27, 0x2e),
            muted: Color32::from_rgb(0x6b, 0x74, 0x7d),
            faint: Color32::from_rgb(0x9a, 0xa2, 0xab),
            accent: Color32::from_rgb(0x18, 0xa0, 0x58),
            accent_strong: Color32::from_rgb(0x14, 0x89, 0x4b),
            danger: Color32::from_rgb(0xd9, 0x53, 0x4f),
            ok: Color32::from_rgb(0x18, 0xa0, 0x58),
            border: Color32::from_rgb(0xeb, 0xed, 0xf0),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: Color32::from_rgb(0x18, 0x1c, 0x22),
            fg: Color32::from_rgb(0xee, 0xf1, 0xf4),
            muted: Color32::from_rgb(0x8b, 0x94, 0x9e),
            faint: Color32::from_rgb(0x62, 0x6c, 0x76),
            accent: Color32::from_rgb(0x2b, 0xb5, 0x6a),
            accent_strong: Color32::from_rgb(0x39, 0xc1, 0x76),
            danger: Color32::from_rgb(0xe0, 0x6b, 0x67),
            ok: Color32::from_rgb(0x2b, 0xb5, 0x6a),
            border: Color32::from_rgba_premultiplied(0xff, 0xff, 0xff, 18),
        }
    }

    pub fn from_dark(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }
}

/// 编辑器排版设置（字号 / 行距 / 字体族）。
#[derive(Clone)]
pub struct FontConfig {
    pub size: f32,
    pub line_scale: f32,
    pub family: FontFamily,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: 13.0,
            line_scale: 1.35,
            family: FontFamily::Monospace,
        }
    }
}

impl FontConfig {
    pub fn font_id(&self) -> egui::FontId {
        egui::FontId::new(self.size, self.family.clone())
    }

    /// 可哈希的判据键（字号 / 行距 / 字体族），供布局缓存失效判断。
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.size.to_bits().hash(&mut h);
        self.line_scale.to_bits().hash(&mut h);
        match &self.family {
            FontFamily::Monospace => 0u8.hash(&mut h),
            FontFamily::Proportional => 1u8.hash(&mut h),
            FontFamily::Name(n) => n.hash(&mut h),
        }
        h.finish()
    }

    /// 实际行高（px）。
    pub fn row_height(&self) -> f32 {
        (self.size * self.line_scale).max(1.0)
    }

    /// 夹到合法范围。
    pub fn clamped(mut self) -> Self {
        if !self.size.is_finite() {
            self.size = Self::default().size;
        }
        if !self.line_scale.is_finite() {
            self.line_scale = Self::default().line_scale;
        }
        self.size = self.size.clamp(8.0, 32.0);
        self.line_scale = self.line_scale.clamp(1.0, 2.0);
        self
    }
}
