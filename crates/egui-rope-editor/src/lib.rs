//! A lightweight, memory-efficient code editor widget for egui.
//!
//! # 设计要点
//!
//! - **rope 存储**：底层 [`ropey::Rope`]，编辑 O(log n)，不拷贝整段文本。
//! - **视口虚拟化**：大文件（> 50 万字符）只对视口附近的行做 galley 排版，
//!   内存与文件大小脱钩（egui 全文排版 5MB ≈ 1.1GB，这里 ~几十 MB）。
//! - **增量更新**：编辑一个字符只 splice 字符数组 + 平移折叠区间，不再全量重建。
//! - **代码折叠 / 搜索 / 行号 / 软换行**：与编辑器一体，不依赖宿主。
//!
//! # 例子
//!
//! ```ignore
//! let mut text = ropey::Rope::from_str("{\n  \"hello\": \"world\"\n}");
//! egui_rope_editor::code_editor(ui, "my-editor", &mut text);
//! ```
//!
//! 需要自定义配色 / 高亮 / 字体时用 [`CodeEditor`] builder。

mod config;
mod editor_core;
mod highlight;

pub use config::{Colors, FontConfig};
pub use highlight::{Highlighter, JsonHighlighter, PlainHighlighter};

use egui::{Response, Ui};
use ropey::Rope;

/// 编辑器 builder：逐项覆盖默认配置，最后 [`CodeEditor::show`]。
pub struct CodeEditor<'a> {
    id_source: &'a str,
    height: Option<f32>,
    wrap: bool,
    colors: Colors,
    font: FontConfig,
    highlighter: &'a dyn Highlighter,
}

impl<'a> CodeEditor<'a> {
    pub fn new(id_source: &'a str) -> Self {
        Self {
            id_source,
            height: None,
            wrap: true,
            colors: Colors::dark(),
            font: FontConfig::default(),
            highlighter: &JsonHighlighter,
        }
    }

    /// 编辑区高度（缺省铺满 `ui.available_height()`）。
    pub fn height(mut self, h: f32) -> Self {
        self.height = Some(h);
        self
    }

    /// 自动换行。大文件下始终软换行（保底），此开关只影响小文件。
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn colors(mut self, colors: Colors) -> Self {
        self.colors = colors;
        self
    }

    pub fn font(mut self, font: FontConfig) -> Self {
        self.font = font;
        self
    }

    /// 语法高亮器（默认 [`JsonHighlighter`]）。
    pub fn highlighter(mut self, highlighter: &'a dyn Highlighter) -> Self {
        self.highlighter = highlighter;
        self
    }

    pub fn show(self, ui: &mut Ui, text: &mut Rope) -> Response {
        let height = self.height.unwrap_or_else(|| ui.available_height());
        editor_core::code_editor(
            ui,
            &self.colors,
            self.id_source,
            text,
            height,
            self.wrap,
            self.font,
            self.highlighter,
        )
    }
}

/// 默认配置的便捷入口：JSON 高亮 + 深色主题 + 自动换行，铺满可用高度。
pub fn code_editor(ui: &mut Ui, id_source: &str, text: &mut Rope) -> Response {
    CodeEditor::new(id_source).show(ui, text)
}
