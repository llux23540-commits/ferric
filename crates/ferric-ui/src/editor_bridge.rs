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
use crate::state::{CellSpan, EditorState, TokenLine, TokenRun};
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

/// 语法着色的种类。目前只有 JSON —— 其它工具的正文（SQL / 正则 / 密文）
/// 要么本来就不该染色，要么词法不是逐行独立的，硬上会给出错的颜色。
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    #[default]
    None,
    Json,
}

/// 编辑区的装饰：差异（逐行种类 + 字符级高亮）与语法着色。
///
/// 下标语义与编辑区一致 —— `kinds` 按**文档行号**索引，`emph` 是
/// `(行, 起始 char, 长度)`。全部在这里被裁到当前视口，与文档多大无关。
#[derive(Default, Clone, Copy)]
pub struct RowDecor<'a> {
    pub kinds: &'a [i32],
    pub emph: &'a [(usize, usize, usize)],
    pub syntax: Syntax,
}

/// 把缓冲区打包成 Slint 的 `EditorState`。
///
/// 只有**可见的那几十行**进这里 —— 这正是虚拟化的出口，布局高度因此恒等于
/// 视口高度，与文档多大无关（Slint 原生 TextEdit 超过约 2190 行会 panic）。
pub fn state_of(buf: &TextBuffer) -> EditorState {
    state_with_decor(buf, RowDecor::default())
}

/// 带差异装饰的版本（对比工具用）。
pub fn state_with_decor(buf: &TextBuffer, decor: RowDecor) -> EditorState {
    let lines: Vec<SharedString> = buf
        .visible_lines()
        .into_iter()
        .map(SharedString::from)
        .collect();

    let cur_row = buf.cursor_view_row();

    // 选区与字符级高亮的横向坐标都是「窄字宽的倍数」（中文一个字宽过一格），
    // UI 侧乘上量出来的窄字宽即得像素。
    let spans: Vec<CellSpan> = buf
        .selection_spans()
        .into_iter()
        .map(|(row, x, w)| CellSpan {
            row: row as i32,
            x,
            w,
        })
        .collect();

    // 行号与折叠标记都按**可见行**对齐：折叠之后行号不再连续
    //（收起 3..9 的话行号槽是 …2, 3, 10, 11…），必须逐行给。
    let rows = buf.visible_rows();
    let line_nos: Vec<i32> = rows.iter().map(|l| (*l + 1) as i32).collect();
    let marks: Vec<i32> = rows.iter().map(|l| buf.fold_mark_of(*l)).collect();
    // `foldable` 决定行号槽要不要让出折叠列 —— 一份没有任何区块的文本
    //（SQL、纯文本）不该白占 16px。
    let foldable = marks.iter().any(|m| *m != 0);

    let kinds: Vec<i32> = rows
        .iter()
        .map(|l| decor.kinds.get(*l).copied().unwrap_or(0))
        .collect();
    let diffed = kinds.iter().any(|k| *k != 0);

    // 字符级高亮裁到视口：行不在这一屏、或整段被横向滚动推出去的都不画。
    // 坐标同样是窄字宽的倍数（`cells_of_range` 负责裁剪与换算）。
    let left = buf.scroll_col();
    let right = left + buf.viewport_cols() + 1;
    let emph: Vec<CellSpan> = decor
        .emph
        .iter()
        .filter_map(|(line, col, len)| {
            let row = rows.iter().position(|l| l == line)?;
            let (x, w) = buf.cells_of_range(*line, *col, *len)?;
            Some(CellSpan {
                row: row as i32,
                x,
                w,
            })
        })
        .collect();

    // 语法着色：**只对这一屏的行**跑词法。每行给出一串片段（文本 + 颜色），
    // UI 侧顺次排出来 —— 不按 char 列绝对定位，因为中文是双宽字符，
    // 「一个 char 一格」会让相邻片段互相压字。
    let tokens: Vec<TokenLine> = if decor.syntax == Syntax::None {
        Vec::new()
    } else {
        rows.iter()
            .map(|line| {
                let text = buf.line_text(*line);
                let chars: Vec<char> = text.chars().collect();
                let runs: Vec<TokenRun> = ferric_core::json::highlight_line(&text)
                    .into_iter()
                    .filter_map(|(col, len, kind)| {
                        // 裁到横向滚动窗口：整段在窗口外的不画，跨边界的切一刀。
                        let (a, b) = (col.max(left), (col + len).min(right));
                        (b > a).then(|| TokenRun {
                            text: SharedString::from(
                                chars[a..b].iter().collect::<String>().as_str(),
                            ),
                            kind: token_kind(kind),
                        })
                    })
                    .collect();
                TokenLine {
                    runs: ModelRc::new(VecModel::from(runs)),
                }
            })
            .collect()
    };
    let highlighted = !tokens.is_empty();

    EditorState {
        lines: ModelRc::new(VecModel::from(lines)),
        first_line: buf.view_first_row() as i32,
        line_nos: ModelRc::new(VecModel::from(line_nos)),
        total_lines: buf.view_total_lines() as i32,
        doc_lines: buf.total_lines() as i32,
        fold_marks: ModelRc::new(VecModel::from(marks)),
        foldable,
        row_kinds: ModelRc::new(VecModel::from(kinds)),
        diffed,
        emph_spans: ModelRc::new(VecModel::from(emph)),
        cursor_line: cur_row.unwrap_or(0) as i32,
        cursor_x: buf.cursor_cells(),
        cursor_visible: cur_row.is_some(),
        tokens: ModelRc::new(VecModel::from(tokens)),
        highlighted,
        selection_spans: ModelRc::new(VecModel::from(spans)),
    }
}

/// `ferric_core::json::Token` → `EditorState.tokens` 的 `kind`。
/// 数值与 `ui/editor.slint` 里的调色板一一对应，改一边就要改另一边。
fn token_kind(t: ferric_core::json::Token) -> i32 {
    use ferric_core::json::Token;
    match t {
        Token::Plain => 0,
        Token::Key => 1,
        Token::Str => 2,
        Token::Num => 3,
        Token::Lit => 4,
        Token::Punct => 5,
    }
}

/// 光标所在整行的文本，**带行尾换行**。
///
/// 带换行是有意的：粘到别处时它是完整的一行，而不是接在上一行末尾。
fn current_line(buf: &TextBuffer) -> String {
    let (line, _) = buf.cursor_line_col();
    let mut s = buf.line_text(line);
    s.push('\n');
    s
}

/// 选中光标所在整行，**连行尾换行一起**。
///
/// `select_line()` 只到行尾（三连击的语义），照它剪切会留下一个空行 ——
/// 而「剪掉这一行」的意思是这一行整个没了。
fn select_current_line(buf: &mut TextBuffer) {
    buf.select_line();
    let (start, end) = buf.selection();
    buf.select_range(start, (end + 1).min(buf.len_chars()));
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
            // 无选区时按「当前行」处理，而不是整篇。
            //
            // 原来 Ctrl+C 在没选区时复制整个文档：习惯性按一下，粘出来是几百 KB
            // 的全文，而用户以为自己复制的是光标那一行（各家编辑器都是这个语义）。
            // 复制整篇有明确入口 —— 工具条上的「复制全文」。
            // Ctrl+X 也一并对齐：原来无选区时它什么都不做，两个键行为不一致。
            'c' => {
                out.copy = Some(if buf.has_selection() {
                    buf.selected_text()
                } else {
                    current_line(buf)
                });
            }
            'x' if !read_only => {
                if !buf.has_selection() {
                    select_current_line(buf);
                }
                out.copy = Some(buf.selected_text());
                buf.backspace();
                out.edited = true;
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
    fn ctrl_c_copies_the_selection_or_the_current_line() {
        let mut b = buf();
        // 无选区 → 当前行（带换行），**不是整篇**：习惯性按一下不该把几百 KB
        // 全文塞进剪贴板。整篇有工具条上的「复制全文」。
        b.click(1, 2.0, false);
        let out = apply_key(&mut b, "c", true, false, false);
        assert_eq!(out.copy.as_deref(), Some("world\n"));
        assert_eq!(b.text(), "hello\nworld\n", "复制不该改内容");

        b.click(0, 0.0, false);
        b.click(0, 5.0, true);
        let out = apply_key(&mut b, "c", true, false, false);
        assert_eq!(out.copy.as_deref(), Some("hello"));
    }

    #[test]
    fn ctrl_x_cuts_the_selection_or_the_whole_current_line() {
        let mut b = buf();
        // 无选区 → 剪掉当前行，**连换行一起**：只删内容会留一个空行，
        // 而「剪掉这一行」的意思是这一行整个没了。
        b.click(0, 3.0, false);
        let out = apply_key(&mut b, "x", true, false, false);
        assert_eq!(out.copy.as_deref(), Some("hello\n"));
        assert_eq!(b.text(), "world\n");
        assert!(out.edited);

        let mut b = buf();
        b.click(0, 0.0, false);
        b.click(0, 5.0, true);
        let out = apply_key(&mut b, "x", true, false, false);
        assert_eq!(out.copy.as_deref(), Some("hello"));
        assert_eq!(b.text(), "\nworld\n");
    }

    #[test]
    fn read_only_pane_never_cuts() {
        // 输出面板可以复制，但 Ctrl+X 一个字都不能动
        let mut b = buf();
        let out = apply_key(&mut b, "x", true, false, true);
        assert!(!out.edited);
        assert!(out.copy.is_none());
        assert_eq!(b.text(), "hello\nworld\n");
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
        b.click(0, 0.0, false); // 光标在第 0 行
        b.scroll_to_line(500); // 视野挪到 500
        let st = state_of(&b);
        assert!(!st.cursor_visible);
    }

    #[test]
    fn diff_decoration_is_clipped_to_the_viewport() {
        // 差异高亮跟正文走同一条虚拟化出口：只有这一屏的行进 Slint，
        // 且坐标是**视口内**行号 —— 用文档行号会把颜色画到别的行上。
        use slint::Model;
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let mut b = TextBuffer::new(&text);
        b.set_viewport(10, 80);
        b.scroll_to_doc_line(500);

        let mut kinds = vec![0; 1001];
        kinds[3] = 1; // 视野外
        kinds[502] = 2; // 视野内第 2 行
        let emph = [(502usize, 5usize, 3usize), (3, 0, 4)];
        let st = state_with_decor(
            &b,
            RowDecor {
                kinds: &kinds,
                emph: &emph,
                ..Default::default()
            },
        );

        assert_eq!(st.row_kinds.row_count(), 10, "逐行种类要与可见行一一对应");
        assert_eq!(st.row_kinds.row_data(2), Some(2));
        assert_eq!(st.row_kinds.row_data(0), Some(0));
        assert!(st.diffed);

        assert_eq!(st.emph_spans.row_count(), 1, "视野外的高亮不该进模型");
        let span = st.emph_spans.row_data(0).expect("这一屏有一处字符级高亮");
        assert_eq!(span.row, 2, "行号必须换算成视口内坐标");
        // 全是窄字符，所以「窄字宽的倍数」与字符数一致
        assert_eq!(span.x, 5.0);
        assert_eq!(span.w, 3.0);
    }

    #[test]
    fn plain_editors_carry_no_decoration_at_all() {
        use slint::Model;
        let st = state_of(&buf());
        assert!(!st.diffed);
        assert!(!st.highlighted, "没开语法着色时不该走片段渲染");
        assert_eq!(st.emph_spans.row_count(), 0);
        assert_eq!(st.tokens.row_count(), 0);
    }

    /// 某一可见行的片段序列，摊成 `(文本, 种类)` 好断言。
    fn runs_of(st: &EditorState, row: usize) -> Vec<(String, i32)> {
        use slint::Model;
        st.tokens
            .row_data(row)
            .expect("这一行应当有片段")
            .runs
            .iter()
            .map(|r| (r.text.to_string(), r.kind))
            .collect()
    }

    #[test]
    fn json_line_is_colored_run_by_run() {
        let mut b = TextBuffer::new("{\n  \"name\": \"ferric\"\n}\n");
        b.set_viewport(4, 40);
        let st = state_with_decor(
            &b,
            RowDecor {
                syntax: Syntax::Json,
                ..Default::default()
            },
        );
        assert!(st.highlighted);
        assert_eq!(
            runs_of(&st, 1),
            vec![
                ("  ".to_owned(), 0),
                ("\"name\"".to_owned(), 1),
                (":".to_owned(), 5),
                (" ".to_owned(), 0),
                ("\"ferric\"".to_owned(), 2),
            ]
        );
    }

    #[test]
    fn json_runs_are_clipped_by_horizontal_scroll() {
        // 片段是顺次排出来的，所以横向滚动必须把左边切掉，
        // 不切的话滚出去的字还画在行首。
        let mut b = TextBuffer::new("{\"name\": \"ferric\"}\n");
        b.set_viewport(4, 6);
        b.scroll_cols_by(3);
        let st = state_with_decor(
            &b,
            RowDecor {
                syntax: Syntax::Json,
                ..Default::default()
            },
        );
        let runs = runs_of(&st, 0);
        assert_eq!(
            runs.first().map(|(t, k)| (t.as_str(), *k)),
            Some(("ame\"", 1)),
            "`\"name\"` 从第 1 列起，滚 3 列后行首应当是 ame\"，实际 {runs:?}"
        );
        // 视口只有 6 列（+1 富余），后面的内容不该整段进模型
        let total: usize = runs.iter().map(|(t, _)| t.chars().count()).sum();
        assert!(total <= 7, "裁过头或没裁：{runs:?}");
    }
}
