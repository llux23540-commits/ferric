//! 视口虚拟化的文本缓冲区 —— Slint 版代码编辑区的地基。
//!
//! # 为什么必须自己做
//!
//! Slint 的原生 `TextEdit` 对**整篇文档**做布局，而 software renderer 的坐标
//! 空间是 i16 物理像素（上限 32767）。实测（`slint-spike` 的 rope probe）：
//!
//! | 行数 | 结果 |
//! |---|---|
//! | ≤ 2190 | 正常 |
//! | ≥ 2195 | **panic**：`euclid` 坐标转换 `unwrap on None` |
//!
//! 2192 行 × 15px 行高 ≈ 32767，边界吻合。且内存放大 **107×**
//!（0.32MB 文本让进程涨 34MB）。
//!
//! 也就是说：**任何**文本框只要被粘进两千多行就会崩掉整个应用 —— 这不是
//! 「JSON 工具的性能问题」，是所有文本类工具的崩溃风险。所以视口虚拟化是
//! 前置条件，不是优化。
//!
//! # 做法
//!
//! - 文本存 [`ropey::Rope`]：插入/删除 O(log n)，不做整篇字符串拷贝；
//! - 只把**可见的那几十行**交给 Slint 渲染 → 布局高度恒等于视口高度，
//!   永远碰不到 i16 上限，且与文档大小无关；
//! - 光标/选区/滚动全在这里算，Slint 侧只按坐标画矩形。
//!
//! 这与 egui 时代的 `egui-rope-editor` 同一套思路（那个 crate 绑在 egui 的
//! `Galley`/`Painter` 上，没法移植），但接口更小：Slint 不需要每帧重建。

use ropey::Rope;

/// 撤销栈的字节上限。超过就丢最旧的 —— 大文件编辑时无上限的撤销栈
/// 本身就是内存泄漏（egui 版踩过，见那一版的 `undo` 限流）。
const UNDO_BUDGET: usize = 4 * 1024 * 1024;

/// 一次可撤销的快照。存整份文本：实现简单且正确，代价由 [`UNDO_BUDGET`] 封顶。
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: usize,
}

/// 光标移动的方向/粒度。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
    /// 按单词左移（Ctrl+Left）。
    WordLeft,
    /// 按单词右移（Ctrl+Right）。
    WordRight,
}

/// 文本缓冲区：rope 存储 + 光标 + 选区 + 滚动位置。
///
/// 所有位置都是**字符索引**（不是字节）—— 中文一个字是一个 char，
/// 按字节切会切碎 UTF-8，这在一个中文界面的工具里是必然踩的坑。
pub struct TextBuffer {
    rope: Rope,
    /// 光标（字符索引）。
    cursor: usize,
    /// 选区锚点。等于 `cursor` 表示没有选区。
    anchor: usize,
    /// 视口顶端的行号（0 基）。
    scroll_line: usize,
    /// 视口能显示多少整行。由 UI 侧按高度算出来后灌进来。
    viewport_lines: usize,
    /// 横向滚动（字符数）。不换行模式下需要。
    scroll_col: usize,
    /// 视口宽度（字符数）。
    viewport_cols: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    undo_bytes: usize,
    /// 内容自上次清零以来是否改过（调用方据此决定要不要重算/落盘）。
    dirty: bool,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl TextBuffer {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            cursor: 0,
            anchor: 0,
            scroll_line: 0,
            viewport_lines: 30,
            scroll_col: 0,
            viewport_cols: 80,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_bytes: 0,
            dirty: false,
        }
    }

    // ——— 读 ———

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// 总行数。空文本算 1 行（有一个空行可以放光标）。
    pub fn total_lines(&self) -> usize {
        self.rope.len_lines().max(1)
    }

    pub fn scroll_line(&self) -> usize {
        self.scroll_line
    }

    pub fn scroll_col(&self) -> usize {
        self.scroll_col
    }

    pub fn viewport_lines(&self) -> usize {
        self.viewport_lines
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    /// 选区的 (起, 止) 字符索引，已排序。无选区时两者相等。
    pub fn selection(&self) -> (usize, usize) {
        if self.cursor <= self.anchor {
            (self.cursor, self.anchor)
        } else {
            (self.anchor, self.cursor)
        }
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection();
        if a == b {
            String::new()
        } else {
            self.rope.slice(a..b).to_string()
        }
    }

    /// 光标的 (行, 列)，都是 0 基字符坐标。
    pub fn cursor_line_col(&self) -> (usize, usize) {
        self.line_col_of(self.cursor)
    }

    fn line_col_of(&self, ch: usize) -> (usize, usize) {
        let ch = ch.min(self.rope.len_chars());
        let line = self.rope.char_to_line(ch);
        let line_start = self.rope.line_to_char(line);
        (line, ch - line_start)
    }

    fn char_of_line_col(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.total_lines().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        // 行长度不含换行符 —— 光标不该停在换行符之后。
        let len = self.line_len(line);
        start + col.min(len)
    }

    /// 某行的字符数（不含行尾换行符）。
    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let l = self.rope.line(line);
        let mut n = l.len_chars();
        // ropey 的 line 带着行尾换行符，去掉（CRLF 去两个）。
        let mut it = l.chars_at(n);
        if let Some('\n') = it.prev() {
            n -= 1;
            if let Some('\r') = it.prev() {
                n -= 1;
            }
        }
        n
    }

    /// 可见行的文本（供 Slint 渲染）。按 `scroll_col` 做横向裁剪。
    ///
    /// 这是虚拟化的出口：**只有这几十行**会进 Slint 的布局，
    /// 所以布局高度恒等于视口高度，与文档多大无关。
    pub fn visible_lines(&self) -> Vec<String> {
        let end = (self.scroll_line + self.viewport_lines).min(self.total_lines());
        (self.scroll_line..end)
            .map(|i| {
                let len = self.line_len(i);
                if self.scroll_col >= len {
                    return String::new();
                }
                let start = self.rope.line_to_char(i) + self.scroll_col;
                let take = (len - self.scroll_col).min(self.viewport_cols + 1);
                self.rope.slice(start..start + take).to_string()
            })
            .collect()
    }

    /// 最长可见行的字符数（横向滚动条用）。
    pub fn max_line_len(&self) -> usize {
        let end = (self.scroll_line + self.viewport_lines).min(self.total_lines());
        (self.scroll_line..end)
            .map(|i| self.line_len(i))
            .max()
            .unwrap_or(0)
    }

    // ——— 视口 ———

    /// UI 侧按控件高度算出能显示几行后调这个。
    pub fn set_viewport(&mut self, lines: usize, cols: usize) {
        self.viewport_lines = lines.max(1);
        self.viewport_cols = cols.max(1);
        self.clamp_scroll();
    }

    /// 滚动 `delta` 行（正 = 往下）。
    pub fn scroll_by(&mut self, delta: i32) {
        let t = self.scroll_line as i64 + delta as i64;
        self.scroll_line = t.max(0) as usize;
        self.clamp_scroll();
    }

    /// 直接把视口顶端设到某一行（滚动条拖动用）。
    pub fn scroll_to_line(&mut self, line: usize) {
        self.scroll_line = line;
        self.clamp_scroll();
    }

    pub fn scroll_cols_by(&mut self, delta: i32) {
        let t = self.scroll_col as i64 + delta as i64;
        self.scroll_col = t.max(0) as usize;
    }

    fn clamp_scroll(&mut self) {
        // 最多滚到「最后一屏」，不允许把内容整个滚出视野之外 ——
        // 那会让用户以为文本没了。
        let max = self.total_lines().saturating_sub(1);
        self.scroll_line = self.scroll_line.min(max);
    }

    /// 把光标滚进视野（每次移动光标/插入后调用）。
    pub fn scroll_to_cursor(&mut self) {
        let (line, col) = self.cursor_line_col();
        if line < self.scroll_line {
            self.scroll_line = line;
        } else if line >= self.scroll_line + self.viewport_lines {
            self.scroll_line = line + 1 - self.viewport_lines;
        }
        if col < self.scroll_col {
            self.scroll_col = col;
        } else if col >= self.scroll_col + self.viewport_cols {
            self.scroll_col = col + 1 - self.viewport_cols;
        }
    }

    // ——— 光标 ———

    /// 点击定位。`line` / `col` 是**视口内**坐标，这里加上滚动偏移。
    pub fn click(&mut self, view_line: usize, view_col: usize, extend: bool) {
        let line = self.scroll_line + view_line;
        let col = self.scroll_col + view_col;
        self.cursor = self.char_of_line_col(line, col);
        if !extend {
            self.anchor = self.cursor;
        }
    }

    pub fn move_cursor(&mut self, m: Motion, extend: bool) {
        let n = self.rope.len_chars();
        let (line, col) = self.cursor_line_col();
        self.cursor = match m {
            Motion::Left => {
                // 有选区时不带 shift 的左移 = 塌缩到选区左端（编辑器通例）
                if !extend && self.has_selection() {
                    self.selection().0
                } else {
                    self.cursor.saturating_sub(1)
                }
            }
            Motion::Right => {
                if !extend && self.has_selection() {
                    self.selection().1
                } else {
                    (self.cursor + 1).min(n)
                }
            }
            Motion::Up => {
                if line == 0 {
                    0
                } else {
                    self.char_of_line_col(line - 1, col)
                }
            }
            Motion::Down => self.char_of_line_col(line + 1, col),
            Motion::LineStart => self.rope.line_to_char(line),
            Motion::LineEnd => self.rope.line_to_char(line) + self.line_len(line),
            Motion::DocStart => 0,
            Motion::DocEnd => n,
            Motion::PageUp => {
                let target = line.saturating_sub(self.viewport_lines);
                self.char_of_line_col(target, col)
            }
            Motion::PageDown => self.char_of_line_col(line + self.viewport_lines, col),
            Motion::WordLeft => self.word_boundary_left(),
            Motion::WordRight => self.word_boundary_right(),
        };
        if !extend {
            self.anchor = self.cursor;
        }
        self.scroll_to_cursor();
    }

    fn word_boundary_left(&self) -> usize {
        let mut i = self.cursor;
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        // 先跳过紧邻左侧的空白，再跳过一整个词
        while i > 0 && !is_word(self.rope.char(i - 1)) {
            i -= 1;
        }
        while i > 0 && is_word(self.rope.char(i - 1)) {
            i -= 1;
        }
        i
    }

    fn word_boundary_right(&self) -> usize {
        let n = self.rope.len_chars();
        let mut i = self.cursor;
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        while i < n && !is_word(self.rope.char(i)) {
            i += 1;
        }
        while i < n && is_word(self.rope.char(i)) {
            i += 1;
        }
        i
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.rope.len_chars();
    }

    /// 选中光标所在的整行（三连击）。
    pub fn select_line(&mut self) {
        let (line, _) = self.cursor_line_col();
        self.anchor = self.rope.line_to_char(line);
        self.cursor = self.anchor + self.line_len(line);
    }

    /// 选中 `[start, end)`（字符索引），并把选区滚进视野。
    ///
    /// 搜索跳转用。越界索引夹到文档范围内 —— 命中位置来自调用方的搜索结果，
    /// 文本可能已经变了，硬索引会 panic。
    pub fn select_range(&mut self, start: usize, end: usize) {
        let n = self.rope.len_chars();
        self.anchor = start.min(n);
        self.cursor = end.min(n);
        self.scroll_to_cursor();
        // 把命中行尽量放到视口中间：搜索结果贴在最后一行上很难看清上下文。
        let (line, _) = self.cursor_line_col();
        let half = self.viewport_lines / 2;
        self.scroll_line = line.saturating_sub(half);
        self.clamp_scroll();
    }

    // ——— 编辑 ———

    fn push_undo(&mut self) {
        let snap = Snapshot {
            text: self.rope.to_string(),
            cursor: self.cursor,
            anchor: self.anchor,
        };
        self.undo_bytes += snap.text.len();
        self.undo.push(snap);
        // 超预算就丢最旧的。无上限的撤销栈在大文件上就是内存泄漏。
        while self.undo_bytes > UNDO_BUDGET && self.undo.len() > 1 {
            let dropped = self.undo.remove(0);
            self.undo_bytes -= dropped.text.len();
        }
        self.redo.clear();
        self.dirty = true;
    }

    /// 删掉选区（若有）。返回是否真的删了东西。
    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.rope.remove(a..b);
        self.cursor = a;
        self.anchor = a;
        true
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.push_undo();
        self.delete_selection();
        self.rope.insert(self.cursor, s);
        self.cursor += s.chars().count();
        self.anchor = self.cursor;
        self.scroll_to_cursor();
    }

    pub fn insert_char(&mut self, c: char) {
        self.push_undo();
        self.delete_selection();
        self.rope.insert_char(self.cursor, c);
        self.cursor += 1;
        self.anchor = self.cursor;
        self.scroll_to_cursor();
    }

    /// Backspace。
    pub fn backspace(&mut self) {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection();
            self.scroll_to_cursor();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        self.rope.remove(self.cursor - 1..self.cursor);
        self.cursor -= 1;
        self.anchor = self.cursor;
        self.scroll_to_cursor();
    }

    /// Delete。
    pub fn delete(&mut self) {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection();
            self.scroll_to_cursor();
            return;
        }
        if self.cursor >= self.rope.len_chars() {
            return;
        }
        self.push_undo();
        self.rope.remove(self.cursor..self.cursor + 1);
        self.scroll_to_cursor();
    }

    /// 整份替换（载入文件、格式化结果回填）。重置光标与滚动。
    pub fn set_text(&mut self, text: &str) {
        self.push_undo();
        self.rope = Rope::from_str(text);
        self.cursor = 0;
        self.anchor = 0;
        self.scroll_line = 0;
        self.scroll_col = 0;
    }

    /// 整份替换但**保留光标与滚动位置**（原地格式化：用户不希望视野跳回顶部）。
    pub fn replace_keeping_view(&mut self, text: &str) {
        self.push_undo();
        let (line, col) = self.cursor_line_col();
        let scroll = self.scroll_line;
        self.rope = Rope::from_str(text);
        self.cursor = self.char_of_line_col(line, col);
        self.anchor = self.cursor;
        self.scroll_line = scroll.min(self.total_lines().saturating_sub(1));
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        let Some(snap) = self.undo.pop() else { return };
        self.undo_bytes = self.undo_bytes.saturating_sub(snap.text.len());
        let cur = Snapshot {
            text: self.rope.to_string(),
            cursor: self.cursor,
            anchor: self.anchor,
        };
        self.redo.push(cur);
        self.rope = Rope::from_str(&snap.text);
        self.cursor = snap.cursor.min(self.rope.len_chars());
        self.anchor = snap.anchor.min(self.rope.len_chars());
        self.dirty = true;
        self.scroll_to_cursor();
    }

    pub fn redo(&mut self) {
        let Some(snap) = self.redo.pop() else { return };
        let cur = Snapshot {
            text: self.rope.to_string(),
            cursor: self.cursor,
            anchor: self.anchor,
        };
        self.undo_bytes += cur.text.len();
        self.undo.push(cur);
        self.rope = Rope::from_str(&snap.text);
        self.cursor = snap.cursor.min(self.rope.len_chars());
        self.anchor = snap.anchor.min(self.rope.len_chars());
        self.dirty = true;
        self.scroll_to_cursor();
    }

    /// 选区在可见范围内的高亮矩形（视口行坐标 + 列区间）。
    ///
    /// 返回 `(视口行, 起列, 列数)`。Slint 侧照这个画矩形 —— 只有可见的那几行
    /// 需要画，与选区跨多少万行无关。
    pub fn selection_spans(&self) -> Vec<(usize, usize, usize)> {
        if !self.has_selection() {
            return Vec::new();
        }
        let (a, b) = self.selection();
        let (la, ca) = self.line_col_of(a);
        let (lb, cb) = self.line_col_of(b);
        let top = self.scroll_line;
        let bottom = self.scroll_line + self.viewport_lines;

        (la.max(top)..=lb.min(bottom.saturating_sub(1)))
            .filter(|l| *l >= top && *l < bottom)
            .map(|l| {
                let start = if l == la { ca } else { 0 };
                let end = if l == lb { cb } else { self.line_len(l) };
                // 空行的选区也要看得见 —— 给一格宽度表示「这一行被选中了」
                let width = end.saturating_sub(start).max(if l < lb { 1 } else { 0 });
                (
                    l - top,
                    start.saturating_sub(self.scroll_col),
                    width,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> TextBuffer {
        TextBuffer::new(s)
    }

    #[test]
    fn only_visible_lines_are_handed_to_the_renderer() {
        // 这是整个组件存在的理由：Slint 的 TextEdit 在 ~2190 行以上会 panic
        //（software renderer 的 i16 坐标空间）。虚拟化后交给渲染层的行数
        // 恒等于视口高度，与文档多大无关。
        let text: String = (0..50_000).map(|i| format!("line {i}\n")).collect();
        let mut b = buf(&text);
        b.set_viewport(40, 100);
        assert_eq!(b.total_lines(), 50_001);
        assert_eq!(b.visible_lines().len(), 40, "交给渲染层的行数必须等于视口高度");

        b.scroll_to_line(49_990);
        assert!(
            b.visible_lines().len() <= 40,
            "滚到末尾也不能超过视口高度"
        );
    }

    #[test]
    fn visible_window_follows_scroll() {
        let mut b = buf("a\nb\nc\nd\ne\nf\n");
        b.set_viewport(2, 80);
        assert_eq!(b.visible_lines(), vec!["a", "b"]);
        b.scroll_by(2);
        assert_eq!(b.visible_lines(), vec!["c", "d"]);
        b.scroll_by(-1);
        assert_eq!(b.visible_lines(), vec!["b", "c"]);
        // 往上滚不过头
        b.scroll_by(-99);
        assert_eq!(b.visible_lines(), vec!["a", "b"]);
    }

    #[test]
    fn scrolling_never_pushes_content_out_of_sight() {
        // 允许滚到内容之外的话，用户会以为文本丢了。
        let mut b = buf("a\nb\nc\n");
        b.set_viewport(2, 80);
        b.scroll_by(9999);
        assert!(!b.visible_lines().is_empty(), "滚到底仍必须有内容可见");
    }

    #[test]
    fn cjk_moves_by_character_not_byte() {
        // 中文一个字 3 字节。按字节走会切碎 UTF-8 —— 在一个中文界面的工具里
        // 这是必然踩的坑，所以全部位置都是 char 索引。
        let mut b = buf("中文测试");
        b.move_cursor(Motion::Right, false);
        assert_eq!(b.cursor_line_col(), (0, 1));
        b.move_cursor(Motion::Right, false);
        assert_eq!(b.cursor_line_col(), (0, 2));
        b.insert_char('X');
        assert_eq!(b.text(), "中文X测试");
    }

    #[test]
    fn line_len_excludes_the_newline() {
        // 光标不该能停在换行符右边 —— 那会显示成「下一行的第 0 列」，
        // 上下移动时列号跳来跳去。
        let b = buf("abc\nde\n");
        assert_eq!(b.line_len(0), 3);
        assert_eq!(b.line_len(1), 2);
    }

    #[test]
    fn line_len_handles_crlf() {
        let b = buf("abc\r\nde\r\n");
        assert_eq!(b.line_len(0), 3, "CRLF 的两个字符都不算进行长度");
        assert_eq!(b.line_len(1), 2);
    }

    #[test]
    fn end_key_lands_before_the_newline() {
        let mut b = buf("abc\ndef\n");
        b.move_cursor(Motion::LineEnd, false);
        assert_eq!(b.cursor_line_col(), (0, 3));
        // 再按一次右移才跨行
        b.move_cursor(Motion::Right, false);
        assert_eq!(b.cursor_line_col(), (1, 0));
    }

    #[test]
    fn vertical_move_keeps_column_when_possible() {
        let mut b = buf("abcdef\nxy\nabcdef\n");
        b.click(0, 5, false);
        assert_eq!(b.cursor_line_col(), (0, 5));
        b.move_cursor(Motion::Down, false);
        // 短行上只能停在行尾
        assert_eq!(b.cursor_line_col(), (1, 2));
    }

    #[test]
    fn plain_left_collapses_selection_instead_of_moving() {
        // 编辑器通例：有选区时不带 shift 的左移是「塌缩到左端」，
        // 而不是「在选区左端再往左一格」。
        let mut b = buf("abcdef");
        b.click(0, 2, false);
        b.click(0, 5, true);
        assert!(b.has_selection());
        b.move_cursor(Motion::Left, false);
        assert_eq!(b.cursor_line_col(), (0, 2));
        assert!(!b.has_selection());
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut b = buf("hello world");
        b.click(0, 0, false);
        b.click(0, 5, true);
        b.insert_str("bye");
        assert_eq!(b.text(), "bye world");
        assert!(!b.has_selection());
    }

    #[test]
    fn undo_redo_restores_text_and_cursor() {
        let mut b = buf("abc");
        b.move_cursor(Motion::DocEnd, false);
        b.insert_str("def");
        assert_eq!(b.text(), "abcdef");
        b.undo();
        assert_eq!(b.text(), "abc");
        b.redo();
        assert_eq!(b.text(), "abcdef");
    }

    #[test]
    fn undo_stack_is_byte_capped() {
        // 无上限的撤销栈在大文件上就是内存泄漏（egui 版踩过）。
        let chunk = "x".repeat(256 * 1024);
        let mut b = buf(&chunk);
        for _ in 0..40 {
            b.move_cursor(Motion::DocEnd, false);
            b.insert_str("y");
        }
        assert!(
            b.undo_bytes <= UNDO_BUDGET + chunk.len(),
            "撤销栈超出预算：{} 字节",
            b.undo_bytes
        );
        assert!(b.can_undo(), "限流之后仍要能撤销至少一步");
    }

    #[test]
    fn replace_keeping_view_does_not_jump_to_top() {
        // 原地格式化：用户在第 500 行按格式化，视野不该跳回顶部。
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let mut b = buf(&text);
        b.set_viewport(20, 80);
        b.scroll_to_line(500);
        b.click(3, 0, false);
        let formatted: String = (0..1000).map(|i| format!("  line {i}\n")).collect();
        b.replace_keeping_view(&formatted);
        assert_eq!(b.scroll_line(), 500, "格式化后视野跳走了");
    }

    #[test]
    fn set_text_resets_the_view() {
        // 载入新文件是另一回事：那时候必须回到顶部。
        let mut b = buf(&(0..100).map(|i| format!("{i}\n")).collect::<String>());
        b.set_viewport(10, 80);
        b.scroll_to_line(50);
        b.set_text("fresh\n");
        assert_eq!(b.scroll_line(), 0);
        assert_eq!(b.cursor_line_col(), (0, 0));
    }

    #[test]
    fn selection_spans_are_clipped_to_the_viewport() {
        // 选中整个 10 万行文档时，只有可见的那几行需要画高亮。
        let text: String = (0..100_000).map(|i| format!("line {i}\n")).collect();
        let mut b = buf(&text);
        b.set_viewport(25, 80);
        b.select_all();
        b.scroll_to_line(40_000);
        let spans = b.selection_spans();
        assert!(
            spans.len() <= 25,
            "高亮矩形数必须受视口约束，实际 {}",
            spans.len()
        );
        assert!(spans.iter().all(|(l, _, _)| *l < 25), "行号必须是视口内坐标");
    }

    #[test]
    fn word_motion_stops_at_boundaries() {
        let mut b = buf("foo bar_baz  qux");
        b.move_cursor(Motion::DocEnd, false);
        b.move_cursor(Motion::WordLeft, false);
        assert_eq!(b.cursor_line_col(), (0, 13), "应停在 qux 开头");
        b.move_cursor(Motion::WordLeft, false);
        assert_eq!(b.cursor_line_col(), (0, 4), "bar_baz 整体算一个词");
    }

    #[test]
    fn cursor_scrolls_into_view_after_moving() {
        let text: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let mut b = buf(&text);
        b.set_viewport(10, 80);
        b.move_cursor(Motion::DocEnd, false);
        let (line, _) = b.cursor_line_col();
        assert!(
            line >= b.scroll_line() && line < b.scroll_line() + 10,
            "光标移动后必须在视野内"
        );
    }

    #[test]
    fn horizontal_clipping_respects_scroll_col() {
        let mut b = buf("0123456789abcdef");
        b.set_viewport(1, 5);
        assert_eq!(b.visible_lines()[0].chars().next(), Some('0'));
        b.scroll_cols_by(10);
        assert_eq!(b.visible_lines()[0].chars().next(), Some('a'));
    }

    #[test]
    fn empty_buffer_has_one_line_for_the_cursor() {
        let b = buf("");
        assert_eq!(b.total_lines(), 1);
        assert_eq!(b.visible_lines().len(), 1);
    }
}