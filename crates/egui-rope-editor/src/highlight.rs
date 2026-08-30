//! 语法高亮：trait + 内置 JSON 实现。

use crate::config::Colors;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// 语法高亮器：把一段文本着色成 [`LayoutJob`]。
///
/// `line_height` 为 `Some` 时按给定行高排版；`None` 用字体自带行高。
pub trait Highlighter {
    fn highlight(&self, text: &str, font_id: &FontId, colors: &Colors, line_height: Option<f32>) -> LayoutJob;
}

/// 空高亮：整段单一前景色（无语法高亮的大文件保底）。
pub struct PlainHighlighter;

impl Highlighter for PlainHighlighter {
    fn highlight(&self, text: &str, font_id: &FontId, colors: &Colors, line_height: Option<f32>) -> LayoutJob {
        let mut job = LayoutJob::default();
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: colors.fg,
                line_height,
                ..Default::default()
            },
        );
        job
    }
}

/// JSON 高亮：键=主色、字符串值=绿、数字=琥珀、true/false=红、null/标点=弱色。
#[derive(Default, Clone, Copy)]
pub struct JsonHighlighter;

impl Highlighter for JsonHighlighter {
    fn highlight(&self, text: &str, font_id: &FontId, colors: &Colors, line_height: Option<f32>) -> LayoutJob {
        let num_col = if colors.dark {
            Color32::from_rgb(0xe0, 0xb0, 0x62)
        } else {
            Color32::from_rgb(0xb0, 0x6f, 0x00)
        };
        let mut job = LayoutJob::default();
        let mk = |c: Color32| TextFormat {
            font_id: font_id.clone(),
            color: c,
            line_height,
            ..Default::default()
        };
        let b = text.as_bytes();
        let n = b.len();
        let mut i = 0usize;
        let mut run = 0usize;
        let is_ws = |c: u8| c == b' ' || c == b'\t' || c == b'\n' || c == b'\r';
        while i < n {
            let c = b[i];
            if c == b'"' {
                if run < i {
                    job.append(&text[run..i], 0.0, mk(colors.muted));
                }
                let start = i;
                i += 1;
                while i < n {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                let end = i.min(n);
                let mut k = end;
                while k < n && is_ws(b[k]) {
                    k += 1;
                }
                let is_key = k < n && b[k] == b':';
                let col = if is_key { colors.accent_strong } else { colors.ok };
                job.append(&text[start..end], 0.0, mk(col));
                run = end;
            } else if c == b'-' || c.is_ascii_digit() {
                if run < i {
                    job.append(&text[run..i], 0.0, mk(colors.muted));
                }
                let start = i;
                i += 1;
                while i < n {
                    let d = b[i];
                    if d.is_ascii_digit() || d == b'.' || d == b'e' || d == b'E' || d == b'+' || d == b'-' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                job.append(&text[start..i], 0.0, mk(num_col));
                run = i;
            } else if c == b't' || c == b'f' || c == b'l' || c == b'n' {
                let rest = &text[i..];
                let (lit, col) = if rest.starts_with("true") || rest.starts_with("false") {
                    (if c == b't' { 4 } else { 5 }, colors.danger)
                } else if rest.starts_with("null") {
                    (4, colors.muted)
                } else {
                    (0, colors.muted)
                };
                if lit > 0 {
                    if run < i {
                        job.append(&text[run..i], 0.0, mk(colors.muted));
                    }
                    job.append(&text[i..i + lit], 0.0, mk(col));
                    i += lit;
                    run = i;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        if run < n {
            job.append(&text[run..], 0.0, mk(colors.muted));
        }
        job
    }
}
