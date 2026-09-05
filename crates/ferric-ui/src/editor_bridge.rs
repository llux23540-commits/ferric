//! 编辑区 ↔ Slint 的桥接：状态打包与按键分派。
//!
//! Slint 侧的 `CodeEditor` 只认一个 `EditorState`（可见行 + 光标 + 选区）和
//! 一组带标识的回调。这里做两件事：
//!
//! 1. [`state_of`]：把 [`TextBuffer`] 打包成 `EditorState`；
//! 2. [`apply_key`]：把 Slint 的原始按键字符翻译成缓冲区操作。
//!
//! 按键分派放在 Rust 而不是 `.slint` 里，是被逼的：原本在 `.slint` 写了
//! 十三个 `if (ev.text == Key.Xxx)` 分支，Slint 编译器在那上面**栈溢出**。
//! 好在功能键就是约定好的码点（见 i-slint-common 的 `key_codes.rs`），
//! 在这边 match 反而更清楚，也顺手把 Ctrl 组合键一起收了。

use crate::editor::{Motion, TextBuffer};
use crate::state::EditorState;
use slint::{ModelRc, SharedString, VecModel};

// Slint 的功能键码点（i-slint-common/key_codes.rs）。
const K_BACKSPACE: char = '\u{0008}';
const K_TAB: char = '\u{0009}';
const K_RETURN: char = '\u{000a}';
const K_ESCAPE: char = '\u{001b}';
const K_DELETE: char = '\u{007f}';
const K_UP: char = '\u{F700}';
const K_DOWN: char = '\u{F701}';
const K_LEFT: char = '\u{F702}';
const K_RIGHT: char = '\u{F703}';
const K_HOME: char = '\u{F729}';
const K_END: char = '\u{F72B}';
const K_PAGE_UP: char = '\u{F72C}';
const K_PAGE_DOWN: char = '\u{F72D}';

/// 一次按键处理的结果，告诉外壳还要不要做别的事。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyOutcome {
    /// 文本被改过（调用方据此重算 / 落盘）。
    pub edited: bool,
    /// 请求把选中文本（或整篇）写进剪贴板。
    pub copy: Option<String>,
    /// 请求从剪贴板粘贴。
    pub paste: bool,
}

/// 把缓冲区打包成 Slint 的 `EditorState`。
///
/// 只有**可见的那几十行**进这里 —— 这正是虚拟化的出口，布局高度因此恒等于
/// 视口高度，与文档多大无关（Slint 原生 TextEdit 超过约 2190 行会 panic）。
pub fn state_of(buf: &TextBuffer) -> EditorState {
    let lines: Vec<SharedString> = buf
        .visible_lines()
        .into_iter()
        .map(SharedString::from)
        .collect();

    let (cur_line, cur_col) = buf.cursor_line_col();
    let top = buf.scroll_line();
    let in_view = cur_line >= top && cur_line < top + buf.viewport_lines();

    let spans: Vec<ModelRc<i32>> = buf
        .selection_spans()
        .into_iter()
        .map(|(line, col, width)| {
            ModelRc::new(VecModel::from(vec![line as i32, col as i32, width as i32]))
        })
        .collect();

    EditorState {
        lines: ModelRc::new(VecModel::from(lines)),
        first_line: top as i32,
        total_lines: buf.total_lines() as i32,
        cursor_line: if in_view { (cur_line - top) as i32 } else { 0 },
        cursor_col: cur_col.saturating_sub(buf.scroll_col()) as i32,
        cursor_visible: in_view,
        selection_spans: ModelRc::new(VecModel::from(spans)),
    }
}

/// 把一次按键作用到缓冲区上。
///
/// `text` 是 Slint 给的原始按键字符：功能键是约定码点，可打印字符就是它本身。
/// `read_only` 的编辑区只处理移动与复制 —— 它是输出面板，用户可以选中/滚动/
/// 复制，但不能改内容（改了也会被下一次重算冲掉，反而更迷惑）。
pub fn apply_key(
    buf: &mut TextBuffer,
    text: &str,
    ctrl: bool,
    shift: bool,
    read_only: bool,
) -> KeyOutcome {
    let mut out = KeyOutcome::default();
    let Some(c) = text.chars().next() else {
        return out;
    };

    // ——— Ctrl 组合键 ———
    if ctrl {
        match c.to_ascii_lowercase() {
            'a' => buf.select_all(),
            'c' => {
                out.copy = Some(if buf.has_selection() {
                    buf.selected_text()
                } else {
                    buf.text()
                });
            }
            'x' if !read_only => {
                if buf.has_selection() {
                    out.copy = Some(buf.selected_text());
                    buf.backspace();
                    out.edited = true;
                }
            }
            'v' if !read_only => out.paste = true,
            'z' if !read_only => {
                buf.undo();
                out.edited = true;
            }
            'y' if !read_only => {
                buf.redo();
                out.edited = true;
            }
            // Ctrl+Home / Ctrl+End 走下面的移动分支
            _ => match c {
                K_HOME => buf.move_cursor(Motion::DocStart, shift),
                K_END => buf.move_cursor(Motion::DocEnd, shift),
                K_LEFT => buf.move_cursor(Motion::WordLeft, shift),
                K_RIGHT => buf.move_cursor(Motion::WordRight, shift),
                _ => {}
            },
        }
        return out;
    }

    // ——— 光标移动（只读也允许：要能选中与滚动） ———
    let motion = match c {
        K_LEFT => Some(Motion::Left),
        K_RIGHT => Some(Motion::Right),
        K_UP => Some(Motion::Up),
        K_DOWN => Some(Motion::Down),
        K_HOME => Some(Motion::LineStart),
        K_END => Some(Motion::LineEnd),
        K_PAGE_UP => Some(Motion::PageUp),
        K_PAGE_DOWN => Some(Motion::PageDown),
        _ => None,
    };
    if let Some(m) = motion {
        buf.move_cursor(m, shift);
        return out;
    }

    if read_only {
        return out;
    }

    // ——— 编辑 ———
    match c {
        K_BACKSPACE => {
            buf.backspace();
            out.edited = true;
        }
        K_DELETE => {
            buf.delete();
            out.edited = true;
        }
        K_RETURN => {
            buf.insert_char('\n');
            out.edited = true;
        }
        K_TAB => {
            // Tab 插两个空格：JSON / YAML 里制表符是坑（不同工具显示宽度不同），
            // 而这几个工具的输出本身就是空格缩进。
            buf.insert_str("  ");
            out.edited = true;
        }
        K_ESCAPE => {}
        // 其余可打印字符。控制字符一律忽略 —— 让它们进文本只会造出看不见的脏数据。
        c if !c.is_control() => {
            buf.insert_char(c);
            out.edited = true;
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf() -> TextBuffer {
        let mut b = TextBuffer::new("hello\nworld\n");
        b.set_viewport(10, 40);
        b
    }

    #[test]
    fn printable_chars_insert() {
        let mut b = buf();
        let out = apply_key(&mut b, "X", false, false, false);
        assert!(out.edited);
        assert_eq!(b.text(), "Xhello\nworld\n");
    }

    #[test]
    fn control_chars_are_dropped_not_inserted() {
        // 让控制字符进文本只会造出看不见的脏数据（复制出去到别处才炸）。
        let mut b = buf();
        let out = apply_key(&mut b, "\u{0007}", false, false, false);
        assert!(!out.edited);
        assert_eq!(b.text(), "hello\nworld\n");
    }

    #[test]
    fn enter_inserts_a_newline_not_the_raw_key_char() {
        let mut b = buf();
        apply_key(&mut b, &K_RETURN.to_string(), false, false, false);
        assert_eq!(b.text(), "\nhello\nworld\n");
    }

    #[test]
    fn tab_inserts_spaces() {
        // 制表符在 JSON/YAML 里是坑（各工具显示宽度不一），统一插空格。
        let mut b = buf();
        apply_key(&mut b, &K_TAB.to_string(), false, false, false);
        assert!(b.text().starts_with("  hello"));
    }

    #[test]
    fn read_only_pane_allows_moving_and_copying_but_not_editing() {
        // 输出面板：能选中/滚动/复制，不能改 —— 改了也会被下次重算冲掉。
        let mut b = buf();
        let before = b.text();

        assert!(!apply_key(&mut b, "X", false, false, true).edited);
        assert!(!apply_key(&mut b, &K_BACKSPACE.to_string(), false, false, true).edited);
        assert!(!apply_key(&mut b, &K_RETURN.to_string(), false, false, true).edited);
        assert_eq!(b.text(), before, "只读面板的内容被改了");

        // 但移动与全选要能用
        apply_key(&mut b, &K_RIGHT.to_string(), false, false, true);
        assert_eq!(b.cursor_line_col(), (0, 1));
        let out = apply_key(&mut b, "a", true, false, true);
        assert!(b.has_selection(), "只读面板必须能全选");
        assert!(!out.edited);
    }

    #[test]
    fn ctrl_c_copies_selection_or_whole_text() {
        let mut b = buf();
        // 无选区 → 复制整篇（用户按 Ctrl+C 想要的是「把结果拿走」）
        let out = apply_key(&mut b, "c", true, false, false);
        assert_eq!(out.copy.as_deref(), Some("hello\nworld\n"));

        b.select_all();
        b.move_cursor(Motion::DocStart, false);
        b.click(0, 0, false);
        b.click(0, 5, true);
        let out = apply_key(&mut b, "c", true, false, false);
        assert_eq!(out.copy.as_deref(), Some("hello"));
    }

    #[test]
    fn ctrl_x_cuts_only_when_there_is_a_selection() {
        let mut b = buf();
        // 没选区时剪切不该把整篇删掉 —— 那是灾难性的误操作
        let out = apply_key(&mut b, "x", true, false, false);
        assert!(!out.edited);
        assert_eq!(b.text(), "hello\nworld\n");

        b.click(0, 0, false);
        b.click(0, 5, true);
        let out = apply_key(&mut b, "x", true, false, false);
        assert_eq!(out.copy.as_deref(), Some("hello"));
        assert_eq!(b.text(), "\nworld\n");
    }

    #[test]
    fn ctrl_z_then_ctrl_y_round_trips() {
        let mut b = buf();
        apply_key(&mut b, "X", false, false, false);
        assert_eq!(b.text(), "Xhello\nworld\n");
        apply_key(&mut b, "z", true, false, false);
        assert_eq!(b.text(), "hello\nworld\n");
        apply_key(&mut b, "y", true, false, false);
        assert_eq!(b.text(), "Xhello\nworld\n");
    }

    #[test]
    fn ctrl_word_motion_uses_word_boundaries() {
        let mut b = TextBuffer::new("foo bar baz");
        b.set_viewport(5, 40);
        b.move_cursor(Motion::DocEnd, false);
        apply_key(&mut b, &K_LEFT.to_string(), true, false, false);
        assert_eq!(b.cursor_line_col(), (0, 8), "Ctrl+Left 应跳到 baz 开头");
    }

    #[test]
    fn ctrl_home_end_jump_to_document_bounds() {
        let mut b = buf();
        apply_key(&mut b, &K_END.to_string(), true, false, false);
        assert_eq!(b.cursor_line_col(), (2, 0), "Ctrl+End 应到文档末尾");
        apply_key(&mut b, &K_HOME.to_string(), true, false, false);
        assert_eq!(b.cursor_line_col(), (0, 0));
    }

    #[test]
    fn shift_arrow_extends_the_selection() {
        let mut b = buf();
        apply_key(&mut b, &K_RIGHT.to_string(), false, true, false);
        apply_key(&mut b, &K_RIGHT.to_string(), false, true, false);
        assert!(b.has_selection());
        assert_eq!(b.selected_text(), "he");
    }

    #[test]
    fn state_hands_over_only_the_visible_window() {
        // 这条是虚拟化的守门人：无论文档多大，交给 Slint 的行数恒等于视口高度。
        use slint::Model;
        let text: String = (0..80_000).map(|i| format!("line {i}\n")).collect();
        let mut b = TextBuffer::new(&text);
        b.set_viewport(25, 80);
        b.scroll_to_line(70_000);

        let st = state_of(&b);
        assert_eq!(st.lines.row_count(), 25);
        assert_eq!(st.total_lines, 80_001);
        assert_eq!(st.first_line, 70_000);
    }

    #[test]
    fn cursor_outside_the_viewport_is_not_drawn() {
        // 光标在视野外还画的话，会在视口顶部留一个假光标。
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let mut b = TextBuffer::new(&text);
        b.set_viewport(10, 80);
        b.click(0, 0, false); // 光标在第 0 行
        b.scroll_to_line(500); // 视野挪到 500
        let st = state_of(&b);
        assert!(!st.cursor_visible);
    }
}
