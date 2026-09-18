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

std::thread_local! {
    /// 宽字符（中日韩、全角标点、emoji）的步进 ÷ 窄字宽。
    ///
    /// 等宽字体只对拉丁字形等宽 —— 中文由回退字体渲染，实际步进宽一截。
    /// 光标 / 选区若按「一个 char 一格」算，中文行上就会与真实字形错开：
    /// 点在这里、光标画在那里、字又插到第三个地方（用户报的「输入异常」）。
    ///
    /// 值由 UI 侧量出来（同字体同字号量 `0` 与 `字` 两个字形）写进来。
    /// 全进程同一套字体，所以是个全局量，不给每个缓冲区各存一份。
    static WIDE_RATIO: std::cell::Cell<f32> = const { std::cell::Cell::new(2.0) };
}

/// UI 侧量完宽字符步进后写进来。越界值忽略 —— 量不出来时保持上一次的值，
/// 别把横向坐标算成 0。
pub fn set_wide_ratio(ratio: f32) {
    if ratio.is_finite() && (1.0..=4.0).contains(&ratio) {
        WIDE_RATIO.with(|r| r.set(ratio));
    }
}

/// 当前宽字符步进（窄字宽的倍数）。
pub fn wide_ratio() -> f32 {
    WIDE_RATIO.with(|r| r.get())
}

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
    /// 是否启用软换行。
    wrap: bool,
    /// 视口顶端在 `scroll_line` 行内的第几个折行（0 基）。`!wrap` 时恒为 0。
    scroll_sub_row: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    undo_bytes: usize,
    /// 已折叠的区间 `(起始行, 结束行)`：起始行照常可见，`起+1..=止` 被隐藏。
    ///
    /// 不变式：按起始行升序、**互不重叠**（折外层时内层被吞掉，见
    /// [`TextBuffer::toggle_fold`]）。视口映射的所有算术都依赖这两条 ——
    /// 允许嵌套的话「第 N 个可见行是文档第几行」就不再是一次线性扫描。
    folds: Vec<(usize, usize)>,
    /// 内容自上次清零以来是否改过（调用方据此决定要不要重算/落盘）。
    dirty: bool,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new("")
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum CharCategory {
    Word,
    Cjk,
    Whitespace,
    Punct,
}

fn is_cjk_char(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{20000}'..='\u{2A6DF}'
        | '\u{F900}'..='\u{FAFF}'
    )
}

fn char_category(c: char) -> CharCategory {
    if is_cjk_char(c) {
        CharCategory::Cjk
    } else if c.is_alphanumeric() || c == '_' {
        CharCategory::Word
    } else if c.is_whitespace() {
        CharCategory::Whitespace
    } else {
        CharCategory::Punct
    }
}

fn is_same_word_char(c: char, cat: CharCategory, target_char: char) -> bool {
    if c == '\n' || c == '\r' {
        return false;
    }
    match cat {
        CharCategory::Word => c.is_alphanumeric() || c == '_',
        CharCategory::Cjk => is_cjk_char(c),
        CharCategory::Whitespace => c.is_whitespace(),
        CharCategory::Punct => {
            let is_delimiter = |ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
                )
            };
            if is_delimiter(target_char) {
                false
            } else {
                char_category(c) == CharCategory::Punct && !is_delimiter(c) && c == target_char
            }
        }
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
            folds: Vec::new(),
            dirty: false,
            wrap: false,
            scroll_sub_row: 0,
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

    pub fn viewport_cols(&self) -> usize {
        self.viewport_cols
    }

    pub fn wrap(&self) -> bool {
        self.wrap
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.scroll_col = 0;
            self.scroll_sub_row = 0;
            self.scroll_to_cursor();
        }
    }

    pub fn wrap_cols(&self) -> usize {
        self.viewport_cols.max(20)
    }

    pub fn sub_rows_of_line(&self, line: usize) -> usize {
        if !self.wrap {
            return 1;
        }
        let len = self.line_len(line);
        if len == 0 {
            1
        } else {
            let cols = self.wrap_cols();
            len.div_ceil(cols)
        }
    }

    pub fn scroll_to_col(&mut self, col: usize) {
        if self.wrap {
            return;
        }
        let max_len = self.max_line_len();
        self.scroll_col = col.min(max_len.saturating_sub(1));
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

    pub fn char_of_line_col(&self, line: usize, col: usize) -> usize {
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

    /// 可见行的文本（供 Slint 渲染）。按 `scroll_col` 做横向裁剪，
    /// 被折叠区间藏起来的行整行跳过。
    ///
    /// 这是虚拟化的出口：**只有这几十行**会进 Slint 的布局，
    /// 所以布局高度恒等于视口高度，与文档多大无关。
    pub fn visible_lines(&self) -> Vec<String> {
        let cols = self.wrap_cols();
        self.visible_rows_info()
            .into_iter()
            .map(|(i, sub)| {
                let mut s = if self.wrap {
                    self.wrapped_line_slice(i, sub, cols)
                } else {
                    self.sliced_line(i)
                };
                let is_last_sub = sub + 1 >= self.sub_rows_of_line(i);
                if is_last_sub && self.folded_end(i).is_some() {
                    if let Some((_, open)) = self.line_opener(i) {
                        s.push_str(" … ");
                        s.push(if open == '{' { '}' } else { ']' });
                    }
                }
                s
            })
            .collect()
    }

    fn wrapped_line_slice(&self, line: usize, sub: usize, cols: usize) -> String {
        let len = self.line_len(line);
        let start_col = sub * cols;
        if start_col >= len {
            return String::new();
        }
        let take = (len - start_col).min(cols);
        let start = self.rope.line_to_char(line) + start_col;
        self.rope.slice(start..start + take).to_string()
    }

    /// 某一行的完整文本（不含行尾换行）。
    ///
    /// 语法着色要拿**整行**做词法：用横向滚动裁过的那份会从字符串中间开始，
    /// 引号配对全错，颜色跟着错。
    pub fn line_text(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        let start = self.rope.line_to_char(line);
        self.rope
            .slice(start..start + self.line_len(line))
            .to_string()
    }

    /// 单行按横向滚动裁剪后的文本。
    fn sliced_line(&self, line: usize) -> String {
        let len = self.line_len(line);
        if self.scroll_col >= len {
            return String::new();
        }
        let start = self.rope.line_to_char(line) + self.scroll_col;
        let take = (len - self.scroll_col).min(self.viewport_cols + 1);
        self.rope.slice(start..start + take).to_string()
    }

    /// 最长可见行的字符数（横向滚动条用）。
    pub fn max_line_len(&self) -> usize {
        (0..self.total_lines())
            .map(|i| self.line_len(i))
            .max()
            .unwrap_or(0)
    }

    // ——— 折叠 ———
    //
    // 折叠只改**视图**：rope 一个字符都不动，隐藏的行仍然在文档里、仍然会被
    // 格式化/搜索/复制看到。这一层要保证的是「第 N 个可见行是文档第几行」
    // 这个换算处处一致 —— 光标、选区、滚动条、点击命中都走同一组函数。

    /// 可见行数（折叠之后的行数）。滚动条比例与视口内换算都用这个，
    /// 文档真实行数仍然是 [`Self::total_lines`]。
    pub fn view_total_lines(&self) -> usize {
        let physical = self.total_lines() - self.hidden_total();
        if !self.wrap {
            return physical;
        }
        let mut extra = 0;
        let cols = self.wrap_cols();
        for l in 0..self.total_lines() {
            if self.folds.iter().any(|(s, e)| *s < l && l <= *e) {
                continue;
            }
            let len = self.line_len(l);
            if len > cols {
                extra += len.div_ceil(cols) - 1;
            }
        }
        physical + extra
    }

    /// 视口顶端是第几个**可见行**（滚动条位置用）。
    pub fn view_first_row(&self) -> usize {
        let base = self
            .scroll_line
            .saturating_sub(self.hidden_before(self.scroll_line));
        if !self.wrap {
            return base;
        }
        let cols = self.wrap_cols();
        let mut extra = 0;
        for l in 0..self.scroll_line {
            if self.folds.iter().any(|(s, e)| *s < l && l <= *e) {
                continue;
            }
            let len = self.line_len(l);
            if len > cols {
                extra += len.div_ceil(cols) - 1;
            }
        }
        base + extra + self.scroll_sub_row
    }

    /// 某一行的折叠标记：`0` = 不是区块开头、`1` = 可折叠、`2` = 已折叠。
    /// 行号槽照它画箭头。
    pub fn fold_mark_of(&self, line: usize) -> i32 {
        if self.folded_end(line).is_some() {
            2
        } else if self.line_opener(line).is_some() {
            1
        } else {
            0
        }
    }

    /// 折叠 / 展开第 `row` 个可见行所在的区块（点行号槽里的箭头）。
    pub fn toggle_fold_view_row(&mut self, row: usize) {
        let line = self.doc_line_from_top(row);
        self.toggle_fold(line);
    }

    /// 折叠 / 展开文档第 `line` 行开头的区块。不是区块开头就什么都不做。
    pub fn toggle_fold(&mut self, line: usize) {
        if let Some(pos) = self.folds.iter().position(|(s, _)| *s == line) {
            self.folds.remove(pos);
            return;
        }
        let Some(end) = self.fold_end_of(line) else {
            return;
        };
        // 折外层时吞掉被它包住的内层 —— `folds` 必须互不重叠。
        self.folds.retain(|(s, e)| *e < line || *s > end);
        self.folds.push((line, end));
        self.folds.sort_unstable();
        // 光标不能留在藏起来的行里：那会让用户打字打在看不见的位置。
        let (cur, _) = self.cursor_line_col();
        if cur > line && cur <= end {
            self.cursor = self.char_of_line_col(line, self.line_len(line));
            self.anchor = self.cursor;
        }
        self.clamp_scroll();
    }

    /// 是否有已折叠的区块。
    pub fn has_folds(&self) -> bool {
        !self.folds.is_empty()
    }

    /// 光标在视口里的行号；不在视口内返回 `None`（Slint 侧据此决定画不画光标）。
    ///
    /// 折叠算在内：藏起来的行不占视觉行，光标落在折叠里就算在折叠头那一行。
    pub fn cursor_view_row(&self) -> Option<usize> {
        let (line, col) = self.cursor_line_col();
        let line = self.prev_visible(line);
        let sub = if self.wrap { col / self.wrap_cols() } else { 0 };
        self.visible_rows_info()
            .iter()
            .position(|&(l, s)| l == line && s == sub)
    }

    // ——— 横向坐标（窄字宽为单位） ———
    //
    // 「一个 char 一格」只在纯拉丁文本里成立。中文由回退字体渲染，步进更宽，
    // 所以光标 / 选区 / 字符级高亮的 x 全部按**窄字宽的倍数**（f32）给 UI：
    // 一个窄字 1.0，一个宽字 [`wide_ratio`]。UI 侧乘上量出来的窄字宽即得像素。

    /// 单个字符占几个窄字宽。
    fn char_cells(&self, c: char) -> f32 {
        match unicode_width::UnicodeWidthChar::width(c) {
            Some(w) if w > 1 => wide_ratio(),
            // 零宽（组合符号）也当 0：它挂在前一个字形上，不占位
            Some(0) => 0.0,
            _ => 1.0,
        }
    }

    /// 行内 `[from, to)` 这一段占几个窄字宽。
    fn cells_between(&self, line: usize, from: usize, to: usize) -> f32 {
        if to <= from || line >= self.rope.len_lines() {
            return 0.0;
        }
        let start = self.rope.line_to_char(line);
        let len = self.line_len(line);
        let (from, to) = (from.min(len), to.min(len));
        self.rope
            .slice(start + from..start + to)
            .chars()
            .map(|c| self.char_cells(c))
            .sum()
    }

    pub fn cursor_cells(&self) -> f32 {
        let (line, col) = self.cursor_line_col();
        if self.wrap {
            let cols = self.wrap_cols();
            let sub = col / cols;
            let start_col = sub * cols;
            self.cells_between(line, start_col, col)
        } else {
            self.cells_between(line, self.scroll_col, col)
        }
    }

    /// 行内某个字符区间在视口里的 `(起点, 宽度)`，都以窄字宽为单位。
    /// 完全被横向滚动推出视野时返回 `None`。
    pub fn cells_of_range(&self, line: usize, from: usize, len: usize) -> Option<(f32, f32)> {
        let base_col = if self.wrap {
            let cols = self.wrap_cols();
            (from / cols) * cols
        } else {
            self.scroll_col
        };
        let span_cols = if self.wrap {
            self.wrap_cols()
        } else {
            self.viewport_cols + 1
        };
        let right = base_col + span_cols;
        let (a, b) = (from.max(base_col), (from + len).min(right));
        if b <= a {
            return None;
        }
        Some((
            self.cells_between(line, base_col, a),
            self.cells_between(line, a, b),
        ))
    }

    /// 视口内的横向位置（窄字宽倍数）落在第几个字符上（点击命中用）。
    ///
    /// 落在字形的后半边就算下一个字符 —— 与所有编辑器的手感一致：
    /// 点一个字的右半边，光标应当停在它后面。
    fn col_at_cells(&self, line: usize, cells: f32) -> usize {
        let len = self.line_len(line);
        let start = self.rope.line_to_char(line);
        let mut at = 0.0f32;
        let mut col = self.scroll_col.min(len);
        while col < len {
            let w = self.char_cells(self.rope.char(start + col));
            if cells < at + w / 2.0 {
                return col;
            }
            at += w;
            col += 1;
        }
        len
    }

    fn col_at_cells_offset(
        &self,
        line: usize,
        start_col: usize,
        max_cols: usize,
        cells: f32,
    ) -> usize {
        let len = self.line_len(line);
        let end_col = (start_col + max_cols).min(len);
        let start = self.rope.line_to_char(line);
        let mut at = 0.0f32;
        let mut col = start_col;
        while col < end_col {
            let w = self.char_cells(self.rope.char(start + col));
            if cells < at + w / 2.0 {
                return col - start_col;
            }
            at += w;
            col += 1;
        }
        end_col - start_col
    }

    /// 展开所有把 `line` 藏起来的折叠（搜索命中落在折叠里时要先露出来）。
    fn reveal(&mut self, line: usize) {
        self.folds.retain(|(s, e)| !(*s < line && line <= *e));
    }

    /// 被折叠藏起来的总行数。
    fn hidden_total(&self) -> usize {
        self.folds.iter().map(|(s, e)| e - s).sum()
    }

    /// `0..line` 里被藏起来的行数。
    fn hidden_before(&self, line: usize) -> usize {
        self.folds
            .iter()
            .take_while(|(s, _)| *s < line)
            .map(|(s, e)| (e + 1).min(line) - (s + 1))
            .sum()
    }

    /// `line` 若是折叠头，返回它的结束行。
    fn folded_end(&self, line: usize) -> Option<usize> {
        self.folds.iter().find(|(s, _)| *s == line).map(|(_, e)| *e)
    }

    /// 从 `line` 往下数的第一个可见行（`line` 本身可见就返回它）。
    fn next_visible(&self, line: usize) -> usize {
        let mut l = line;
        while let Some(end) = self
            .folds
            .iter()
            .find(|(s, e)| *s < l && l <= *e)
            .map(|(_, e)| *e)
        {
            l = end + 1;
        }
        l
    }

    /// 从 `line` 往上数的第一个可见行（藏起来的行归到它的折叠头）。
    fn prev_visible(&self, line: usize) -> usize {
        let mut l = line;
        while let Some(start) = self
            .folds
            .iter()
            .find(|(s, e)| *s < l && l <= *e)
            .map(|(s, _)| *s)
        {
            l = start;
        }
        l
    }

    /// 从 `from` 往上退 `rows` 个可见行。
    fn back_visible(&self, from: usize, rows: usize) -> usize {
        let mut l = self.prev_visible(from);
        for _ in 0..rows {
            if l == 0 {
                break;
            }
            l = self.prev_visible(l - 1);
        }
        l
    }

    /// 从 `from` 往下走 `rows` 个可见行（走到末尾就停在最后一个可见行）。
    fn forward_visible(&self, from: usize, rows: usize) -> usize {
        let total = self.total_lines();
        let mut l = self.prev_visible(from.min(total.saturating_sub(1)));
        for _ in 0..rows {
            let next = self.next_visible(l + 1);
            if next >= total {
                break;
            }
            l = next;
        }
        l
    }

    /// `[a, b)` 之间的可见行数。
    fn view_rows_between(&self, a: usize, b: usize) -> usize {
        if b <= a {
            return 0;
        }
        (b - a) - (self.hidden_before(b) - self.hidden_before(a))
    }

    /// 当前视口里那几十个可见行的文档行号（0 基）。
    ///
    /// 行号槽的数字、折叠标记、选区高亮全部照它对齐 —— 折叠之后
    /// 「第 N 个视觉行 = 文档第 N 行」不再成立，只有这一处知道真实映射。
    /// 视口内当前可见的 `(文档行, 折行号)` 序列。
    pub fn visible_rows_info(&self) -> Vec<(usize, usize)> {
        let total = self.total_lines();
        let mut out = Vec::with_capacity(self.viewport_lines);
        let mut l = self.next_visible(self.scroll_line);
        let mut sub = if l == self.scroll_line {
            self.scroll_sub_row
        } else {
            0
        };

        while out.len() < self.viewport_lines && l < total {
            let n_sub = self.sub_rows_of_line(l);
            while sub < n_sub && out.len() < self.viewport_lines {
                out.push((l, sub));
                sub += 1;
            }
            sub = 0;
            l = self.next_visible(l + 1);
        }
        out
    }

    /// 当前视口里那几十个可见行的文档行号（0 基）。
    ///
    /// 行号槽的数字、折叠标记、选区高亮全部照它对齐 —— 折叠之后
    /// 「第 N 个视觉行 = 文档第 N 行」不再成立，只有这一处知道真实映射。
    pub fn visible_rows(&self) -> Vec<usize> {
        self.visible_rows_info()
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    /// 视口第 `rows` 行对应的文档行（越界夹到最后一个可见行）。
    fn doc_line_from_top(&self, rows: usize) -> usize {
        if self.wrap {
            self.visible_rows_info()
                .get(rows)
                .map(|(l, _)| *l)
                .unwrap_or_else(|| self.total_lines().saturating_sub(1))
        } else {
            self.forward_visible(self.scroll_line, rows)
        }
    }

    /// 第 `row` 个可见行（从文档开头数）对应的文档行。
    fn doc_of_view_row(&self, row: usize) -> usize {
        let mut left = row;
        let mut line = 0usize;
        for &(s, e) in &self.folds {
            if s < line {
                continue;
            }
            let span = s - line; // line..s 这一段全可见
            if left <= span {
                return line + left;
            }
            left -= span + 1; // 跨过折叠头那一行
            line = e + 1;
        }
        (line + left).min(self.total_lines().saturating_sub(1))
    }

    /// 这一行末尾是不是 `{` / `[`（区块开头）。返回该字符的索引与字符本身。
    ///
    /// 只看行末的非空白字符，所以 `"a": {},` 这种一行写完的不算可折叠 ——
    /// 与 VS Code 的判断一致。
    fn line_opener(&self, line: usize) -> Option<(usize, char)> {
        if line + 1 >= self.total_lines() {
            return None; // 最后一行开的括号底下没有内容可收
        }
        let start = self.rope.line_to_char(line);
        let mut idx = start + self.line_len(line);
        let mut it = self.rope.chars_at(idx);
        while idx > start {
            let c = it.prev()?;
            idx -= 1;
            if c.is_whitespace() {
                continue;
            }
            return if c == '{' || c == '[' {
                Some((idx, c))
            } else {
                None
            };
        }
        None
    }

    /// 区块的结束行 —— 行末那个 `{` / `[` 的配对行。
    ///
    /// 一次前向扫描（识别字符串与转义，否则 `"{"` 这种值会把配对算错）。
    /// **只在用户点折叠箭头时调用**，不进每帧路径：最坏情况要扫到文档末尾。
    fn fold_end_of(&self, line: usize) -> Option<usize> {
        let (open_idx, open) = self.line_opener(line)?;
        let close = if open == '{' { '}' } else { ']' };
        let mut depth = 0i32;
        let mut in_str = false;
        let mut esc = false;
        for (idx, c) in (open_idx..).zip(self.rope.chars_at(open_idx)) {
            if in_str {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                }
            } else if c == '"' {
                in_str = true;
            } else if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    let end = self.rope.char_to_line(idx);
                    return (end > line).then_some(end);
                }
            }
        }
        None
    }

    /// 编辑之后修正折叠区间：跨过编辑点的折叠作废，之后的整体位移。
    ///
    /// 不这么做的后果是折叠区间指向错的行 —— 表现为「改完一行，别处莫名
    /// 少了几行」。`at_line` 是编辑发生的行，`before` 是编辑前的总行数。
    fn reconcile_folds(&mut self, at_line: usize, before: usize) {
        if self.folds.is_empty() {
            return;
        }
        self.folds
            .retain(|(s, e)| !(*s <= at_line && at_line <= *e));
        let delta = self.total_lines() as isize - before as isize;
        if delta == 0 {
            return;
        }
        for f in &mut self.folds {
            if f.0 > at_line {
                f.0 = f.0.saturating_add_signed(delta);
                f.1 = f.1.saturating_add_signed(delta);
            }
        }
    }

    // ——— 视口 ———

    /// UI 侧按控件高度算出能显示几行后调这个。
    pub fn set_viewport(&mut self, lines: usize, cols: usize) {
        self.viewport_lines = lines.max(1);
        self.viewport_cols = cols.max(1);
        self.clamp_scroll();
    }

    /// 滚动 `delta` **可见行**（正 = 往下）。折叠区间整块跳过。
    pub fn scroll_by(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        if delta > 0 {
            for _ in 0..delta {
                let n_sub = self.sub_rows_of_line(self.scroll_line);
                if self.wrap && self.scroll_sub_row + 1 < n_sub {
                    self.scroll_sub_row += 1;
                } else {
                    let next = self.forward_visible(self.scroll_line, 1);
                    if next > self.scroll_line {
                        self.scroll_line = next;
                        self.scroll_sub_row = 0;
                    } else {
                        break;
                    }
                }
            }
        } else {
            for _ in 0..(-delta) {
                if self.wrap && self.scroll_sub_row > 0 {
                    self.scroll_sub_row -= 1;
                } else if self.scroll_line > 0 {
                    self.scroll_line = self.prev_visible(self.scroll_line - 1);
                    self.scroll_sub_row = if self.wrap {
                        self.sub_rows_of_line(self.scroll_line).saturating_sub(1)
                    } else {
                        0
                    };
                } else {
                    break;
                }
            }
        }
        self.clamp_scroll();
    }

    /// 把视口顶端设到第 `row` 个**可见行**（滚动条拖动用）。
    pub fn scroll_to_line(&mut self, row: usize) {
        if !self.wrap {
            self.scroll_line = self.doc_of_view_row(row);
            self.scroll_sub_row = 0;
        } else {
            let mut left = row;
            let mut line = 0usize;
            let total = self.total_lines();
            while line < total {
                line = self.next_visible(line);
                if line >= total {
                    break;
                }
                let n_sub = self.sub_rows_of_line(line);
                if left < n_sub {
                    self.scroll_line = line;
                    self.scroll_sub_row = left;
                    self.clamp_scroll();
                    return;
                }
                left -= n_sub;
                line += 1;
            }
            self.scroll_line = total.saturating_sub(1);
            self.scroll_sub_row = 0;
        }
        self.clamp_scroll();
    }

    /// 把视口顶端设到**文档**某一行。
    pub fn scroll_to_doc_line(&mut self, line: usize) {
        self.scroll_line = line;
        self.scroll_sub_row = 0;
        self.clamp_scroll();
    }

    pub fn scroll_cols_by(&mut self, delta: i32) {
        if self.wrap {
            return;
        }
        let t = self.scroll_col as i64 + delta as i64;
        self.scroll_col = t.max(0) as usize;
    }

    fn clamp_scroll(&mut self) {
        let total_view = self.view_total_lines();
        let last_top = total_view.saturating_sub(self.viewport_lines);
        if !self.wrap {
            let row = self.view_rows_between(0, self.scroll_line).min(last_top);
            self.scroll_line = self.doc_of_view_row(row);
            self.scroll_line = self.prev_visible(self.scroll_line);
            self.scroll_sub_row = 0;
        } else {
            let cur_row = self.view_first_row();
            if cur_row > last_top {
                self.scroll_to_line(last_top);
            }
            self.scroll_line = self.prev_visible(self.scroll_line);
            let n_sub = self.sub_rows_of_line(self.scroll_line);
            self.scroll_sub_row = self.scroll_sub_row.min(n_sub.saturating_sub(1));
        }
    }

    /// 把光标滚进视野（每次移动光标/插入后调用）。
    pub fn scroll_to_cursor(&mut self) {
        let (line, col) = self.cursor_line_col();
        let line = self.prev_visible(line);
        if self.wrap {
            self.scroll_col = 0;
            let cols = self.wrap_cols();
            let sub = col / cols;
            let is_before =
                line < self.scroll_line || (line == self.scroll_line && sub < self.scroll_sub_row);
            if is_before {
                self.scroll_line = line;
                self.scroll_sub_row = sub;
            } else {
                let in_view = self
                    .visible_rows_info()
                    .iter()
                    .any(|&(l, s)| l == line && s == sub);
                if !in_view {
                    self.scroll_line = line;
                    self.scroll_sub_row = sub.saturating_sub(self.viewport_lines.saturating_sub(1));
                }
            }
            self.clamp_scroll();
        } else {
            self.scroll_sub_row = 0;
            if line < self.scroll_line {
                self.scroll_line = line;
            } else if self.view_rows_between(self.scroll_line, line) >= self.viewport_lines {
                self.scroll_line = self.back_visible(line, self.viewport_lines - 1);
            }
            self.clamp_scroll();
            if col < self.scroll_col {
                self.scroll_col = col;
            } else if col >= self.scroll_col + self.viewport_cols {
                self.scroll_col = col + 1 - self.viewport_cols;
            }
        }
    }

    // ——— 光标 ———

    pub fn click(&mut self, view_line: usize, cells: f32, extend: bool) {
        let rows = self.visible_rows_info();
        let (line, sub) = rows
            .get(view_line)
            .copied()
            .unwrap_or((self.total_lines().saturating_sub(1), 0));
        let col = if self.wrap {
            let cols = self.wrap_cols();
            let offset = self.col_at_cells_offset(line, sub * cols, cols, cells.max(0.0));
            sub * cols + offset
        } else {
            self.col_at_cells(line, cells.max(0.0))
        };
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
                if self.wrap {
                    let cols = self.wrap_cols();
                    let sub = col / cols;
                    let col_in_row = col % cols;
                    if sub > 0 {
                        let target_sub = sub - 1;
                        let target_col = (target_sub * cols + col_in_row).min(self.line_len(line));
                        self.char_of_line_col(line, target_col)
                    } else if line > 0 {
                        let prev = self.prev_visible(line - 1);
                        let prev_n_sub = self.sub_rows_of_line(prev);
                        let target_sub = prev_n_sub.saturating_sub(1);
                        let target_col = (target_sub * cols + col_in_row).min(self.line_len(prev));
                        self.char_of_line_col(prev, target_col)
                    } else {
                        0
                    }
                } else if line == 0 {
                    0
                } else {
                    self.char_of_line_col(self.prev_visible(line - 1), col)
                }
            }
            Motion::Down => {
                if self.wrap {
                    let cols = self.wrap_cols();
                    let sub = col / cols;
                    let col_in_row = col % cols;
                    let n_sub = self.sub_rows_of_line(line);
                    if sub + 1 < n_sub {
                        let target_sub = sub + 1;
                        let target_col = (target_sub * cols + col_in_row).min(self.line_len(line));
                        self.char_of_line_col(line, target_col)
                    } else {
                        let next = self.forward_visible(line, 1);
                        if next > line {
                            let target_col = col_in_row.min(self.line_len(next));
                            self.char_of_line_col(next, target_col)
                        } else {
                            self.char_of_line_col(line, self.line_len(line))
                        }
                    }
                } else {
                    self.char_of_line_col(self.forward_visible(line, 1), col)
                }
            }
            Motion::LineStart => self.rope.line_to_char(line),
            Motion::LineEnd => self.rope.line_to_char(line) + self.line_len(line),
            Motion::DocStart => 0,
            Motion::DocEnd => n,
            Motion::PageUp => {
                let target = self.back_visible(line, self.viewport_lines);
                self.char_of_line_col(target, col)
            }
            Motion::PageDown => {
                let target = self.forward_visible(line, self.viewport_lines);
                self.char_of_line_col(target, col)
            }
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

    /// 选中光标所在的词 / 字符序列（双击）。
    ///
    /// 英文/数字/下划线、CJK 汉字、连续空白、符号标点各自独立成组扩张，
    /// 严格限制在当前行内，不跨行。
    pub fn select_word(&mut self) {
        let n = self.rope.len_chars();
        if n == 0 {
            return;
        }
        let (line, col) = self.cursor_line_col();
        let line_len = self.line_len(line);
        if line_len == 0 {
            return;
        }
        let line_start = self.rope.line_to_char(line);
        let target_col = if col >= line_len {
            line_len.saturating_sub(1)
        } else {
            col
        };
        let ch = self.rope.char(line_start + target_col);
        let cat = char_category(ch);

        let mut left = target_col;
        while left > 0 {
            let prev = self.rope.char(line_start + left - 1);
            if !is_same_word_char(prev, cat, ch) {
                break;
            }
            left -= 1;
        }

        let mut right = target_col + 1;
        while right < line_len {
            let next = self.rope.char(line_start + right);
            if !is_same_word_char(next, cat, ch) {
                break;
            }
            right += 1;
        }

        self.anchor = line_start + left;
        self.cursor = line_start + right;
    }

    /// 选中 `[start, end)`（字符索引），并把选区滚进视野。
    ///
    /// 搜索跳转用。越界索引夹到文档范围内 —— 命中位置来自调用方的搜索结果，
    /// 文本可能已经变了，硬索引会 panic。
    pub fn select_range(&mut self, start: usize, end: usize) {
        let n = self.rope.len_chars();
        self.anchor = start.min(n);
        self.cursor = end.min(n);
        // 命中落在折叠里就先展开：否则「跳过去了」但屏幕上什么都没变。
        let (line, _) = self.cursor_line_col();
        self.reveal(line);
        self.scroll_to_cursor();
        // 把命中行/折行尽量放到视口中间：搜索结果贴在最后一行上很难看清上下文。
        let half = self.viewport_lines / 2;
        if self.wrap {
            let (_, col) = self.cursor_line_col();
            let sub = col / self.wrap_cols();
            self.scroll_line = line;
            self.scroll_sub_row = sub.saturating_sub(half);
        } else {
            self.scroll_line = self.back_visible(line, half);
        }
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
        let (at, before) = self.edit_origin();
        self.push_undo();
        self.delete_selection();
        self.rope.insert(self.cursor, s);
        self.cursor += s.chars().count();
        self.anchor = self.cursor;
        self.reconcile_folds(at, before);
        self.scroll_to_cursor();
    }

    pub fn insert_char(&mut self, c: char) {
        let (at, before) = self.edit_origin();
        self.push_undo();
        self.delete_selection();
        self.rope.insert_char(self.cursor, c);
        self.cursor += 1;
        self.anchor = self.cursor;
        self.reconcile_folds(at, before);
        self.scroll_to_cursor();
    }

    /// 编辑发生的位置：`(光标所在行, 当前总行数)`。折叠区间的修正要它。
    fn edit_origin(&self) -> (usize, usize) {
        (self.cursor_line_col().0, self.total_lines())
    }

    /// Backspace。
    pub fn backspace(&mut self) {
        let (at, before) = self.edit_origin();
        if self.has_selection() {
            self.push_undo();
            self.delete_selection();
            self.reconcile_folds(at, before);
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
        self.reconcile_folds(at, before);
        self.scroll_to_cursor();
    }

    /// Delete。
    pub fn delete(&mut self) {
        let (at, before) = self.edit_origin();
        if self.has_selection() {
            self.push_undo();
            self.delete_selection();
            self.reconcile_folds(at, before);
            self.scroll_to_cursor();
            return;
        }
        if self.cursor >= self.rope.len_chars() {
            return;
        }
        self.push_undo();
        self.rope.remove(self.cursor..self.cursor + 1);
        self.reconcile_folds(at, before);
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
        // 整篇换过了，旧的折叠区间指向的行早已不是同一行。
        self.folds.clear();
    }

    /// 整份替换但**保留光标与滚动位置**（原地格式化：用户不希望视野跳回顶部）。
    pub fn replace_keeping_view(&mut self, text: &str) {
        self.push_undo();
        let (line, col) = self.cursor_line_col();
        let scroll = self.scroll_line;
        self.rope = Rope::from_str(text);
        self.folds.clear();
        self.cursor = self.char_of_line_col(line, col);
        self.anchor = self.cursor;
        self.scroll_line = scroll.min(self.total_lines().saturating_sub(1));
        self.scroll_sub_row = 0;
        self.clamp_scroll();
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
        self.folds.clear();
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
        self.folds.clear();
        self.dirty = true;
        self.scroll_to_cursor();
    }

    /// 选区在可见范围内的高亮矩形。
    ///
    /// 返回 `(视口行, 起点, 宽度)`，后两者以**窄字宽**为单位（见
    /// [`Self::cells_of_range`]）。只有可见的那几行需要画，与选区跨多少万行无关。
    pub fn selection_spans(&self) -> Vec<(usize, f32, f32)> {
        if !self.has_selection() {
            return Vec::new();
        }
        let (a, b) = self.selection();
        let (la, ca) = self.line_col_of(a);
        let (lb, cb) = self.line_col_of(b);

        let cols = self.wrap_cols();
        self.visible_rows_info()
            .into_iter()
            .enumerate()
            .filter(|(_, (l, _))| *l >= la && *l <= lb)
            .filter_map(|(row_idx, (l, sub))| {
                let n_sub = self.sub_rows_of_line(l);
                let is_last_sub = sub + 1 >= n_sub;
                let (row_start_col, row_end_col) = if self.wrap {
                    let s = sub * cols;
                    let e = (s + cols).min(self.line_len(l));
                    (s, e)
                } else {
                    (0, self.line_len(l))
                };

                let sel_start = if l == la { ca } else { 0 };
                let sel_end = if l == lb { cb } else { self.line_len(l) };

                let start = sel_start.max(row_start_col).min(row_end_col);
                let end = sel_end.max(row_start_col).min(row_end_col);

                let is_past_line = l < lb;
                if start < end {
                    let (x, w) = self.cells_of_range(l, start, end - start)?;
                    let w = if is_past_line && is_last_sub {
                        w + 1.0
                    } else {
                        w
                    };
                    Some((row_idx, x, w))
                } else if is_past_line && is_last_sub && sel_start >= self.line_len(l) {
                    let from_col = if self.wrap {
                        row_start_col
                    } else {
                        self.scroll_col
                    };
                    let x = self.cells_between(l, from_col, row_end_col);
                    Some((row_idx, x, 1.0))
                } else {
                    None
                }
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
        assert_eq!(
            b.visible_lines().len(),
            40,
            "交给渲染层的行数必须等于视口高度"
        );

        b.scroll_to_line(49_990);
        assert!(b.visible_lines().len() <= 40, "滚到末尾也不能超过视口高度");
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
    fn the_last_screen_stays_full() {
        // 「最后一屏」而不是「最后一行」：滚到底只剩一行正文加一大片空白时，
        // 用户报的是「我的文本呢」。
        let doc: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let mut b = buf(&doc);
        b.set_viewport(30, 80);
        b.scroll_by(9999);
        assert_eq!(b.visible_lines().len(), 30, "滚到底仍要铺满一屏");
    }

    #[test]
    fn pasting_a_whole_document_shows_its_tail_not_a_blank_page() {
        // 用户报的「粘贴没粘全」：Ctrl+A、Ctrl+V 之后光标停在末行，
        // `scroll_to_cursor` 把末行顶到第一排 —— 屏幕上只剩那一行。
        let mut b = buf("old\n");
        b.set_viewport(30, 80);
        b.select_all();
        b.insert_str(&(0..165).map(|i| format!("line {i}\n")).collect::<String>());

        let rows = b.visible_lines();
        assert_eq!(rows.len(), 30, "粘完必须还是满满一屏");
        assert_eq!(rows[29].trim_end(), "", "末行（空行）在最后一排");
        assert_eq!(rows[0].trim_end(), "line 136");
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
    fn cjk_lines_use_real_glyph_widths_for_the_caret() {
        // 用户报的「输入异常」就是这条：中文行上光标画在别处、字插到第三个
        // 地方。根因是横向坐标按「一个 char 一格」算，而中文字形宽一截。
        set_wide_ratio(2.0);
        let mut b = buf("  \"城市\": \"上海\",");
        b.set_viewport(4, 40);

        // 光标移到行尾：2 空格 + 4 个窄字符（"":  两个引号一个冒号一个空格…）
        // 逐字算太绕，直接与「等宽假设」对比：中文行上两者必须不同。
        b.move_cursor(Motion::LineEnd, false);
        let (_, col) = b.cursor_line_col();
        assert!(
            b.cursor_cells() > col as f32,
            "有中文的行，光标位置必须比字符数更靠右（{} vs {col}）",
            b.cursor_cells()
        );

        // 纯 ASCII 行则两者相等 —— 不能因为改了中文而把普通行也算歪。
        let mut a = buf("  \"port\": 8080");
        a.set_viewport(4, 40);
        a.move_cursor(Motion::LineEnd, false);
        assert_eq!(a.cursor_cells(), a.cursor_line_col().1 as f32);
    }

    #[test]
    fn clicking_a_cjk_line_lands_on_the_glyph_under_the_pointer() {
        // 点在第 4 个字形上就该停在第 4 个字符前后，而不是被宽度换算带偏。
        set_wide_ratio(2.0);
        let mut b = buf("ab中文cd");
        b.set_viewport(2, 40);

        // a b 各 1 格，两个中文各 2 格 → "中" 占 [2,4)，"文" 占 [4,6)
        b.click(0, 2.1, false); // 落在「中」的左半边
        assert_eq!(b.cursor_line_col().1, 2, "应当停在「中」之前");
        b.click(0, 3.9, false); // 落在「中」的右半边
        assert_eq!(b.cursor_line_col().1, 3, "应当停在「中」之后");
        b.click(0, 6.1, false); // 落在 c 上
        assert_eq!(b.cursor_line_col().1, 4);
        b.click(0, 99.0, false); // 远超行尾 → 夹到行尾
        assert_eq!(b.cursor_line_col().1, 6);
    }

    #[test]
    fn selection_width_counts_wide_glyphs_twice() {
        set_wide_ratio(2.0);
        let mut b = buf("ab中文cd");
        b.set_viewport(2, 40);
        b.select_range(2, 4); // 选中「中文」
        let spans = b.selection_spans();
        assert_eq!(spans.len(), 1);
        let (row, x, w) = spans[0];
        assert_eq!(row, 0);
        assert_eq!(x, 2.0, "前面是 a b 两个窄字符");
        assert_eq!(w, 4.0, "两个中文 = 四格宽");
    }

    #[test]
    fn wide_ratio_rejects_nonsense_values() {
        // 量不出来（字体没回退、宽度 0）时保持上一次的值 —— 否则横向坐标
        // 会整列塌到 0，界面上表现为「光标永远在行首」。
        set_wide_ratio(2.0);
        set_wide_ratio(0.0);
        assert_eq!(wide_ratio(), 2.0);
        set_wide_ratio(f32::NAN);
        assert_eq!(wide_ratio(), 2.0);
        set_wide_ratio(1.667);
        assert_eq!(wide_ratio(), 1.667);
        set_wide_ratio(2.0); // 复位，别影响同线程的其它测试
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
        b.click(0, 5.0, false);
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
        b.click(0, 2.0, false);
        b.click(0, 5.0, true);
        assert!(b.has_selection());
        b.move_cursor(Motion::Left, false);
        assert_eq!(b.cursor_line_col(), (0, 2));
        assert!(!b.has_selection());
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut b = buf("hello world");
        b.click(0, 0.0, false);
        b.click(0, 5.0, true);
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
    fn drag_selection_in_json_text() {
        let text =
            "{\n  \"items\": [\n    {\n      \"id\": 1,\n      \"name\": \"行-1\"\n    }\n  ]\n}";
        let mut b = buf(text);
        b.set_viewport(20, 80);

        // JSON 中行内拖动：在 `      "id": 1,`（第 4 行，行号 3）上从 0 拖到超出行末的大 cells（如 100）
        // 防御性检查：超出列宽自动夹取至整行字符数，不越界、不含换行符
        b.click(3, 0.0, false);
        b.click(3, 100.0, true);
        assert_eq!(b.selected_text(), "      \"id\": 1,");

        // 跨行拖动：从第 3 行行首拖到第 4 行行首，应完整包含第 3 行及其换行符
        b.click(3, 0.0, false);
        b.click(4, 0.0, true);
        assert_eq!(b.selected_text(), "      \"id\": 1,\n");

        // 含中文双宽字符行（第 5 行，行号 4 `      "name": "行-1"`）：拖选到行末
        b.click(4, 0.0, false);
        b.click(4, 120.0, true);
        assert_eq!(b.selected_text(), "      \"name\": \"行-1\"");
    }

    #[test]
    fn drag_selection_in_sql_text() {
        let sql = "SELECT\n  id,\n  name\nFROM users;\n";
        let mut b = buf(sql);
        b.set_viewport(20, 80);

        // 单行行内拖选超出末尾：夹取至 "SELECT"（不含换行）
        b.click(0, 0.0, false);
        b.click(0, 80.0, true);
        assert_eq!(b.selected_text(), "SELECT");

        // 跨行拖选第 0 行到第 1 行：包含换行符
        b.click(0, 0.0, false);
        b.click(1, 0.0, true);
        assert_eq!(b.selected_text(), "SELECT\n");
    }

    #[test]
    fn drag_selection_in_plain_and_diff_text() {
        let plain = "line 0\nline 1\nline 7\nline 8\n";
        let mut b = buf(plain);
        b.set_viewport(20, 80);

        // 对比工具样本复现：在 `line 7`（第 3 行，行号 2）行内拖动
        // 行内拖选 0 到 50 cells：取到 "line 7"（6 字符，不含换行）
        b.click(2, 0.0, false);
        b.click(2, 50.0, true);
        assert_eq!(b.selected_text(), "line 7");

        // 跨行拖选至下一行行首：包含换行，共 7 字符 "line 7\n"
        b.click(2, 0.0, false);
        b.click(3, 0.0, true);
        assert_eq!(b.selected_text(), "line 7\n");

        // 在行末空白区点击未拖动：无选区，anchor == cursor
        b.click(2, 50.0, false);
        assert_eq!(b.selected_text(), "");
        assert!(!b.has_selection());
    }

    #[test]
    fn defensive_click_and_drag_boundaries() {
        let mut b = buf("abc\ndef\n");
        b.set_viewport(10, 40);

        // 负数 cells 自动夹取到 0，不 panic
        b.click(0, -100.0, false);
        assert_eq!(b.cursor_line_col(), (0, 0));

        // 超大行号自动夹取到最后一行，不越界
        b.click(99999, 0.0, false);
        let (line, _) = b.cursor_line_col();
        assert!(line <= b.total_lines());

        // 空缓冲区防御：点击与拖选不 panic，selected_text 为空
        let mut empty = buf("");
        empty.click(0, 0.0, false);
        empty.click(0, 50.0, true);
        assert_eq!(empty.selected_text(), "");
    }

    #[test]
    fn select_word_alphanumeric_and_underscore() {
        let mut b = buf("hello_world 12345 foo");
        b.set_viewport(10, 80);

        // 点击在 "hello_world" 的第 3 列 ('l')，双击选词
        b.click(0, 3.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "hello_world");

        // 点击在 "12345" 的第 13 列 ('3')，双击选词
        b.click(0, 13.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "12345");

        // 点击在 "foo" 的第 18 列 ('f')，双击选词
        b.click(0, 18.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "foo");
    }

    #[test]
    fn select_word_cjk_characters() {
        set_wide_ratio(2.0);
        let mut b = buf("let msg = \"你好世界\";\n");
        b.set_viewport(10, 80);

        // 选词：点击在 "好"（x 坐标在引号后面）
        // "let msg = \"" 是 11 字符 (11 窄字宽)
        // "你" 占 2 宽 (11..13), "好" 占 2 宽 (13..15)
        b.click(0, 14.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "你好世界");

        // 双击在定界符引号 '"' 上，仅选中该定界字符
        b.click(0, 10.2, false);
        b.select_word();
        assert_eq!(b.selected_text(), "\"");
        // 双击在变量名 "msg" 上
        b.click(0, 5.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "msg");
    }

    #[test]
    fn select_word_whitespace_and_punctuation() {
        let mut b = buf("a ===   b");
        b.set_viewport(10, 80);

        // 双击在连续运算符 "===" 上，选中全段连续符号
        b.click(0, 3.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "===");

        // 双击在空白区上，选中连续空白
        b.click(0, 6.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "   ");
    }

    #[test]
    fn select_word_at_line_end_and_empty_buffer() {
        let mut b = buf("alpha beta");
        b.set_viewport(10, 80);

        // 光标落在行末超出处（如 cells = 50.0），双击选中行末词
        b.click(0, 50.0, false);
        b.select_word();
        assert_eq!(b.selected_text(), "beta");

        // 空缓冲区双击选词，防御无 panic，无选区
        let mut empty = buf("");
        empty.select_word();
        assert_eq!(empty.selected_text(), "");
        assert!(!empty.has_selection());
    }

    #[test]
    fn multiline_selection_spans_include_empty_lines_and_newlines() {
        let mut b = buf("hello\n\nworld\n");
        b.set_viewport(10, 80);

        // 跨越空行选择：第 0 行 0 列到第 2 行末尾 "world" 字符后（未包含第 2 行行尾换行符）
        b.click(0, 0.0, false);
        b.click(2, 5.0, true);
        let spans = b.selection_spans();
        assert_eq!(
            spans.len(),
            3,
            "第 0 行、第 1 行（空行）、第 2 行都应有高亮矩形"
        );

        // 第 0 行："hello\n" (5 文本宽度 + 1 换行标记 = 6.0)
        assert_eq!(spans[0].0, 0);
        assert_eq!(spans[0].2, 6.0);

        // 第 1 行：空行换行标记 (宽度 1.0)
        assert_eq!(spans[1].0, 1);
        assert_eq!(spans[1].2, 1.0);

        // 第 2 行："world" (未选中行尾换行符，宽度为 5.0)
        assert_eq!(spans[2].0, 2);
        assert_eq!(spans[2].2, 5.0);
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
        b.click(3, 0.0, false);
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
        assert!(
            spans.iter().all(|(l, _, _)| *l < 25),
            "行号必须是视口内坐标"
        );
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

    // ——— 折叠 ———

    /// 一份格式化过的 JSON：第 1 行开 `"items": [`，第 5 行收 `]`。
    fn folded_sample() -> TextBuffer {
        let mut b = buf("{\n  \"items\": [\n    1,\n    2,\n    3\n  ],\n  \"tail\": 9\n}");
        b.set_viewport(20, 80);
        b
    }

    #[test]
    fn folding_hides_the_block_but_keeps_the_document() {
        let mut b = folded_sample();
        assert_eq!(b.fold_mark_of(1), 1, "`\"items\": [` 应当可折叠");
        b.toggle_fold(1);

        // 藏起来的是区块内部**连同配对的那一行**（收起的括号画在折叠头上，
        // 与 VS Code 一致）；折叠头本身还在，文档一个字符都没变。
        assert_eq!(b.visible_rows(), vec![0, 1, 6, 7]);
        assert_eq!(b.view_total_lines(), 4);
        assert_eq!(b.total_lines(), 8, "折叠只改视图，不改文档");
        assert!(b.text().contains("    2,"), "被折叠的行仍在文本里");
        assert_eq!(b.fold_mark_of(1), 2, "折叠头的标记要翻成「已折叠」");

        // 折叠头那一行画出「… ]」，否则收起来的行看着跟普通行没差别。
        assert!(
            b.visible_lines()[1].ends_with(" … ]"),
            "折叠头要带收起标记，实际是 {:?}",
            b.visible_lines()[1]
        );

        b.toggle_fold(1);
        assert_eq!(b.view_total_lines(), 8, "再点一次要完全展开");
    }

    #[test]
    fn clicking_below_a_fold_lands_on_the_line_the_user_sees() {
        // 这是折叠最容易出错的地方：视觉第 2 行已经不是文档第 2 行。
        let mut b = folded_sample();
        b.toggle_fold(1);
        b.click(2, 0.0, false); // 视觉第 3 行 = 文档第 6 行（`  "tail": 9`）
        assert_eq!(b.cursor_line_col().0, 6);
        b.click(3, 0.0, false);
        assert_eq!(b.cursor_line_col().0, 7);
    }

    #[test]
    fn arrow_down_steps_over_a_folded_block() {
        let mut b = folded_sample();
        b.toggle_fold(1);
        b.click(1, 0.0, false); // 光标放到折叠头
        b.move_cursor(Motion::Down, false);
        assert_eq!(
            b.cursor_line_col().0,
            6,
            "下一行必须是折叠之后看得见的那一行"
        );
        b.move_cursor(Motion::Up, false);
        assert_eq!(b.cursor_line_col().0, 1);
    }

    #[test]
    fn braces_inside_strings_do_not_decide_the_fold_end() {
        // 字符串里的括号不算配对，否则 `"{"` 这种值会把折叠范围算短一截。
        let mut b = buf("{\n  \"a\": \"{\",\n  \"b\": \"}\",\n  \"c\": 1\n}");
        b.set_viewport(20, 80);
        b.toggle_fold(0);
        assert_eq!(b.visible_rows(), vec![0], "整份对象应当收成一行");
    }

    #[test]
    fn one_line_blocks_are_not_foldable() {
        let b = buf("{\n  \"a\": {},\n  \"b\": []\n}");
        assert_eq!(b.fold_mark_of(1), 0, "`{{}}` 一行写完，没有可收的内容");
        assert_eq!(b.fold_mark_of(2), 0);
    }

    #[test]
    fn editing_inside_a_folded_block_drops_that_fold() {
        // 行号会移位，留着旧区间就会藏错行 —— 表现为「改一行，别处少几行」。
        let mut b = folded_sample();
        b.toggle_fold(1);
        assert!(b.has_folds());
        b.click(1, 12.0, false); // 折叠头上打字
        b.insert_char(' ');
        assert!(!b.has_folds(), "编辑跨过折叠区间就要作废它");
        assert_eq!(b.view_total_lines(), b.total_lines());
    }

    #[test]
    fn editing_above_a_fold_shifts_it_instead_of_dropping_it() {
        let mut b = folded_sample();
        b.toggle_fold(1);
        b.click(0, 1.0, false); // 第 0 行末尾回车，下面所有行 +1
        b.insert_char('\n');
        assert!(b.has_folds(), "编辑点在折叠之前，折叠应当保留");
        assert_eq!(
            b.visible_rows(),
            vec![0, 1, 2, 7, 8],
            "折叠区间要跟着位移，藏的还是同一段内容"
        );
    }

    #[test]
    fn search_jump_reveals_a_hit_inside_a_fold() {
        let mut b = folded_sample();
        b.toggle_fold(1);
        let at = b.text().find('2').expect("样本里有 2");
        b.select_range(at, at + 1);
        assert!(
            !b.has_folds(),
            "命中在折叠里就要先展开，否则跳过去什么也看不见"
        );
        assert!(b.cursor_view_row().is_some(), "光标必须落在视口内");
    }

    #[test]
    fn reformatting_clears_stale_folds() {
        let mut b = folded_sample();
        b.toggle_fold(1);
        b.replace_keeping_view("{\n  \"x\": 1\n}");
        assert!(!b.has_folds(), "整份换过之后旧的折叠区间已经没有意义");
        assert_eq!(b.view_total_lines(), 3);
    }

    #[test]
    fn scrollbar_metrics_follow_the_folded_view() {
        // 滚动条的刻度是可见行，拖动回传的也是可见行号 —— 两边必须同一套坐标。
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut b = buf(&format!("{{\n{text}}}"));
        b.set_viewport(10, 80);
        b.toggle_fold(0);
        assert_eq!(b.view_total_lines(), 1);
        assert_eq!(b.view_first_row(), 0);

        b.toggle_fold(0);
        b.scroll_to_line(20);
        assert_eq!(b.view_first_row(), 20, "没有折叠时可见行号 = 文档行号");
    }

    #[test]
    fn soft_wrap_breaks_long_line_into_sub_rows() {
        let text = "a".repeat(200);
        let mut b = buf(&text);
        b.set_viewport(10, 50); // wrap_cols = 50
        b.set_wrap(true);

        assert_eq!(b.sub_rows_of_line(0), 4, "200 字符按 50 列折成 4 个视觉行");
        assert_eq!(b.view_total_lines(), 4);
        let lines = b.visible_lines();
        assert_eq!(lines.len(), 4);
        for l in &lines {
            assert_eq!(l.len(), 50);
        }
    }

    #[test]
    fn soft_wrap_cursor_down_navigates_sub_rows() {
        let text = "a".repeat(200);
        let mut b = buf(&text);
        b.set_viewport(10, 50);
        b.set_wrap(true);

        assert_eq!(b.cursor, 0);
        // 向下移动光标：行内折行步进
        b.move_cursor(Motion::Down, false);
        assert_eq!(b.cursor, 50);
        b.move_cursor(Motion::Down, false);
        assert_eq!(b.cursor, 100);
        b.move_cursor(Motion::Up, false);
        assert_eq!(b.cursor, 50);
    }

    #[test]
    fn soft_wrap_click_on_sub_row_lands_on_correct_char() {
        let text = "0123456789".repeat(20); // 200 字符
        let mut b = buf(&text);
        b.set_viewport(10, 50);
        b.set_wrap(true);

        // 点击第 1 个子行（第二行）的第 10 格：字符索引应为 50 + 10 = 60
        b.click(1, 10.0, false);
        assert_eq!(b.cursor, 60);
    }

    #[test]
    fn horizontal_scroll_to_col_and_max_line_len() {
        let text = "x".repeat(200);
        let mut b = buf(&text);
        b.set_viewport(10, 50);
        b.set_wrap(false);

        assert_eq!(b.max_line_len(), 200);
        assert_eq!(b.scroll_col(), 0);

        b.scroll_to_col(100);
        assert_eq!(b.scroll_col(), 100);
    }

    #[test]
    fn ultra_long_single_line_json_soft_wrap_browsing() {
        // 100,000 字符单行 JSON
        let text = "{\"id\":1,\"data\":\"".to_string() + &"x".repeat(99_982) + "\"}";
        assert_eq!(text.len(), 100_000);
        let mut b = buf(&text);
        b.set_viewport(30, 100);
        b.set_wrap(true);

        assert_eq!(
            b.sub_rows_of_line(0),
            1000,
            "100,000 字符按 100 列折为 1000 个视觉行"
        );
        assert_eq!(b.view_total_lines(), 1000);

        // 滚动到第 500 个视觉行
        b.scroll_by(500);
        assert_eq!(b.view_first_row(), 500);

        // 获取视口可见行
        let lines = b.visible_lines();
        assert_eq!(lines.len(), 30, "视口行数应为 30");
        for l in &lines {
            assert_eq!(l.len(), 100, "每个折行行宽为 100 字符");
        }

        b.select_range(50_000, 50_250);
        assert_eq!(b.selected_text().len(), 250);
        let spans = b.selection_spans();
        assert_eq!(spans.len(), 3, "跨越 3 个子行");
    }
}
