//! 编辑器字体设置（字号 / 字重 / 行距）。

use egui::FontFamily;
use serde::{Deserialize, Serialize};

/// 编辑区的排版设置（字号 / 字重 / 行距）。
///
/// 单独拎出来而不是沿用全局 `TextStyle::Monospace`：JSON 要么是密密麻麻几千行、
/// 要么是需要逐字核对的密钥串，合适的字号与行距因人因内容而异；而这只该影响这块
/// 编辑区，不该动到整个界面的字号。
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FontCfg {
    /// 字号（px）。
    pub size: f32,
    /// 用中黑（JetBrains Mono Medium）而不是常规字重 —— 小字号下细体容易发虚。
    pub medium: bool,
    /// 行距倍数（相对字号）。
    pub line_scale: f32,
}

impl Default for FontCfg {
    fn default() -> Self {
        // 与改动前的观感一致：egui 默认 Monospace 正是 13px；
        // 1.35 是 JetBrains Mono 在这个字号下比较耐看的密度。
        Self {
            size: 13.0,
            medium: false,
            line_scale: 1.35,
        }
    }
}

impl FontCfg {
    /// 字号范围。下限保证标点还分得清，上限避免一屏放不下几行。
    pub const MIN_SIZE: f32 = 10.0;
    pub const MAX_SIZE: f32 = 22.0;
    /// 行距档位：紧凑 / 标准 / 宽松。
    pub const LINE_SCALES: [(f32, &'static str); 3] =
        [(1.15, "紧凑"), (1.35, "标准"), (1.6, "宽松")];

    pub fn font_id(&self) -> egui::FontId {
        let family = if self.medium {
            FontFamily::Name(crate::fonts::MONO_MEDIUM.into())
        } else {
            FontFamily::Monospace
        };
        egui::FontId::new(self.size, family)
    }

    /// 夹到合法范围。草稿是用户可写的 ron，脏值不能直接拿去排版。
    pub fn clamped(mut self) -> Self {
        if !self.size.is_finite() {
            self.size = Self::default().size;
        }
        if !self.line_scale.is_finite() {
            self.line_scale = Self::default().line_scale;
        }
        self.size = self.size.clamp(Self::MIN_SIZE, Self::MAX_SIZE);
        self.line_scale = self.line_scale.clamp(1.0, 2.0);
        self
    }
}
