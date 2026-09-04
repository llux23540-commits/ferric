//! Lucide 图标字体的字形常量。
//!
//! 码点对照 Lucide `info.json`（`lucide-static`），与设计原型所用图标一致。
//! Slint 侧用 `font-family: "lucide"`（见 `ui/theme.slint` 的 `font-icons`）
//! 加上这里的字符渲染图标；Rust 侧只提供常量，不再构造富文本。

/// Lucide 图标字体族名。与 `ui/theme.slint` 的 `font-icons` 必须一致。
pub const FAMILY: &str = crate::fonts::LUCIDE_FAMILY;

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
// 预留字形：当前没有视图用到，但属于已内嵌的 Lucide 子集，保留备用。
#[allow(dead_code)]
pub const CHEVRON_DOWN: char = '\u{e06d}';
pub const CHEVRON_RIGHT: char = '\u{e06f}';
pub const FILE_DOWN: char = '\u{e318}';
pub const TRASH_2: char = '\u{e18e}';
pub const ERASER: char = '\u{e28f}';
pub const REFRESH_CW: char = '\u{e145}';
pub const UNDO_2: char = '\u{e2a1}';
pub const REDO_2: char = '\u{e2a0}';
pub const ALIGN_LEFT: char = '\u{e185}';
// 预留字形：当前没有视图用到，但属于已内嵌的 Lucide 子集，保留备用。
#[allow(dead_code)]
pub const LIST_TREE: char = '\u{e408}';
pub const INDENT_INCREASE: char = '\u{e108}';
/// Lucide `text-wrap` —— 自动换行开关。
pub const WRAP_TEXT: char = '\u{e248}';
/// Lucide `fold-vertical` —— 把多行压缩成一行。
pub const FOLD_VERTICAL: char = '\u{e43c}';
/// Lucide `type` —— 字重 / 字体。
pub const TYPE_ICON: char = '\u{e198}';
/// Lucide `a-large-small` —— 字号 / 排版设置。
pub const FONT_SIZE: char = '\u{e587}';
/// Lucide `align-vertical-space-around` —— 行距。
pub const LINE_HEIGHT: char = '\u{e27a}';
/// Lucide `rotate-ccw` —— 恢复默认。
pub const ROTATE_CCW: char = '\u{e148}';
pub const FOLDER_OPEN: char = '\u{e247}';
pub const PLUS: char = '\u{e13d}';
pub const ARROW_UP_A_Z: char = '\u{e41a}';
pub const QUOTE: char = '\u{e239}';
pub const CIRCLE_ALERT: char = '\u{e077}';
pub const CIRCLE_CHECK: char = '\u{e226}';
