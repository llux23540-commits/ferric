//! Lucide 图标字体的字形常量与辅助。
//!
//! 码点对照 Lucide `info.json`（`lucide-static`），与设计原型所用图标一致。
//! 用 [`text`] 生成一段图标 `RichText`，以 [`crate::fonts::LUCIDE_FAMILY`] 族渲染。

use crate::fonts::LUCIDE_FAMILY;
use egui::{Color32, FontFamily, RichText};

/// Lucide 图标字体族。
pub fn family() -> FontFamily {
    FontFamily::Name(LUCIDE_FAMILY.into())
}

/// 生成一个图标 `RichText`（指定字号与颜色）。
pub fn text(ch: char, size: f32, color: Color32) -> RichText {
    RichText::new(ch).family(family()).size(size).color(color)
}

// ---- 字形常量（对照 Lucide info.json）----
pub const CODE: char = '\u{e093}';
pub const BRACES: char = '\u{e36a}';
pub const CLOCK: char = '\u{e087}';
pub const DATABASE: char = '\u{e0ad}';
pub const CREDIT_CARD: char = '\u{e0aa}';
pub const KEY: char = '\u{e0fd}';
pub const LOCK: char = '\u{e10b}';
pub const SHIELD_CHECK: char = '\u{e1ff}';
pub const TERMINAL: char = '\u{e181}';
pub const LIST_CHECKS: char = '\u{e1d0}';
pub const BOX: char = '\u{e061}';
pub const MOON: char = '\u{e11e}';
pub const SUN: char = '\u{e178}';
pub const INFO: char = '\u{e0f9}';
pub const SETTINGS: char = '\u{e154}';
pub const X: char = '\u{e1b2}';
pub const MINUS: char = '\u{e11c}';
pub const SQUARE: char = '\u{e167}';
pub const HEART: char = '\u{e0f2}';
pub const SEARCH: char = '\u{e151}';
pub const COPY: char = '\u{e09e}';
pub const CHECK: char = '\u{e06c}';
pub const GIT_COMPARE: char = '\u{e359}';
pub const CHEVRON_DOWN: char = '\u{e06d}';
pub const CHEVRON_RIGHT: char = '\u{e06f}';
pub const FILE_DOWN: char = '\u{e318}';
pub const TRASH_2: char = '\u{e18e}';
pub const ERASER: char = '\u{e28f}';
pub const REFRESH_CW: char = '\u{e145}';
pub const UNDO_2: char = '\u{e2a1}';
pub const REDO_2: char = '\u{e2a0}';
pub const ALIGN_LEFT: char = '\u{e185}';
pub const LIST_TREE: char = '\u{e408}';
pub const INDENT_INCREASE: char = '\u{e108}';
pub const WRAP_TEXT: char = '\u{e248}';
pub const FOLDER_OPEN: char = '\u{e247}';
pub const PLUS: char = '\u{e13d}';
pub const ARROW_UP_A_Z: char = '\u{e41a}';
pub const QUOTE: char = '\u{e239}';
pub const CIRCLE_ALERT: char = '\u{e077}';
pub const CIRCLE_CHECK: char = '\u{e226}';
