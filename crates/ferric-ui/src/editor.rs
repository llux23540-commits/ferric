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

    /// 可见行的文本（供 Slint 渲染）。按 `scroll_col` 做横向裁剪，
    /// 被折叠区间藏起来的行整行跳过。
    ///
    /// 这是虚拟化的出口：**只有这几十行**会进 Slint 的布局，
    /// 所以布局高度恒等于视口高度，与文档多大无关。
    pub fn visible_lines(&self) -> Vec<String> {
        self.visible_rows()
            .into_iter()
            .map(|i| {
                let mut s = self.sliced_line(i);
                // 折叠头补一个「… }」：不然收起来的那一行看着跟普通行一样，
                // 用户不知道底下还藏着东西。
                if self.folded_end(i).is_some() {
                    if let Some((_, open)) = self.line_opener(i) {
                        s.push_str(" … ");
                        s.push(if open == '{' { '}' } else { ']' });
                    }
                }
                s
            })
            .collect()
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
        self.visible_rows()
            .into_iter()
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
        self.total_lines() - self.hidden_total()
    }

    /// 视口顶端是第几个**可见行**（滚动条位置用）。
    pub fn view_first_row(&self) -> usize {
        self.scroll_line - self.hidden_before(self.scroll_line)
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
        let (line, _) = self.cursor_line_col();
        let line = self.prev_visible(line);
        if line < self.scroll_line {
            return None;
        }
        let row = self.view_rows_between(self.scroll_line, line);
        (row < self.viewport_lines).then_some(row)
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
    pub fn visible_rows(&self) -> Vec<usize> {
        let total = self.total_lines();
        let mut out = Vec::with_capacity(self.viewport_lines);
        let mut l = self.next_visible(self.scroll_line);
        while out.len() < self.viewport_lines && l < total {
            out.push(l);
            l = self.next_visible(l + 1);
        }
        out
    }

    /// 视口第 `rows` 行对应的文档行（越界夹到最后一个可见行）。
    fn doc_line_from_top(&self, rows: usize) -> usize {
        self.forward_visible(self.scroll_line, rows)
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
        self.scroll_line = if delta >= 0 {
            self.forward_visible(self.scroll_line, delta as usize)
        } else {
            self.back_visible(self.scroll_line, (-delta) as usize)
        };
        self.clamp_scroll();
    }

    /// 把视口顶端设到第 `row` 个**可见行**（滚动条拖动用 —— 滚动条的刻度是
    /// 可见行数 [`Self::view_total_lines`]，不是文档行数）。
    pub fn scroll_to_line(&mut self, row: usize) {
        self.scroll_line = self.doc_of_view_row(row);
        self.clamp_scroll();
    }

    /// 把视口顶端设到**文档**某一行。
    ///
    /// 与 [`Self::scroll_to_line`] 的区别是刻度：那个收的是可见行序号
    ///（滚动条给的），这个收的是文档行号（对比工具的左右同步滚动用）。
    pub fn scroll_to_doc_line(&mut self, line: usize) {
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
        // 顶端必须落在可见行上：停在被折叠藏起来的行上，第一屏会画成空白。
        self.scroll_line = self.prev_visible(self.scroll_line);
    }

    /// 把光标滚进视野（每次移动光标/插入后调用）。
    pub fn scroll_to_cursor(&mut self) {
        let (line, col) = self.cursor_line_col();
        let line = self.prev_visible(line);
        if line < self.scroll_line {
            self.scroll_line = line;
        } else if self.view_rows_between(self.scroll_line, line) >= self.viewport_lines {
            // 距离按**可见行**算：折叠之后「相差 300 行」可能只有 3 行的视觉距离。
            self.scroll_line = self.back_visible(line, self.viewport_lines - 1);
        }
        if col < self.scroll_col {
            self.scroll_col = col;
        } else if col >= self.scroll_col + self.viewport_cols {
            self.scroll_col = col + 1 - self.viewport_cols;
        }
    }

    // ——— 光标 ———

    /// 点击定位。`line` / `col` 是**视口内**坐标，这里换算成文档坐标。
    pub fn click(&mut self, view_line: usize, view_col: usize, extend: bool) {
        let line = self.doc_line_from_top(view_line);
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
            // 上/下与翻页都按**可见行**走：折叠起来的区块整块跳过，
            // 否则光标会掉进看不见的行里（表现为「按一下方向键光标没了」）。
            Motion::Up => {
                if line == 0 {
                    0
                } else {
                    self.char_of_line_col(self.prev_visible(line - 1), col)
                }
            }
            Motion::Down => self.char_of_line_col(self.forward_visible(line, 1), col),
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
        // 把命中行尽量放到视口中间：搜索结果贴在最后一行上很难看清上下文。
        let half = self.viewport_lines / 2;
        self.scroll_line = self.back_visible(line, half);
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

        // 按视口里的可见行走：折叠藏起来的行不画，后面的行往上顶。
        self.visible_rows()
            .into_iter()
            .enumerate()
            .filter(|(_, l)| *l >= la && *l <= lb)
            .map(|(row, l)| {
                let start = if l == la { ca } else { 0 };
                let end = if l == lb { cb } else { self.line_len(l) };
                // 空行的选区也要看得见 —— 给一格宽度表示「这一行被选中了」
                let width = end.saturating_sub(start).max(if l < lb { 1 } else { 0 });
                (row, start.saturating_sub(self.scroll_col), width)
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
        b.click(2, 0, false); // 视觉第 3 行 = 文档第 6 行（`  "tail": 9`）
        assert_eq!(b.cursor_line_col().0, 6);
        b.click(3, 0, false);
        assert_eq!(b.cursor_line_col().0, 7);
    }

    #[test]
    fn arrow_down_steps_over_a_folded_block() {
        let mut b = folded_sample();
        b.toggle_fold(1);
        b.click(1, 0, false); // 光标放到折叠头
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
        b.click(1, 12, false); // 折叠头上打字
        b.insert_char(' ');
        assert!(!b.has_folds(), "编辑跨过折叠区间就要作废它");
        assert_eq!(b.view_total_lines(), b.total_lines());
    }

    #[test]
    fn editing_above_a_fold_shifts_it_instead_of_dropping_it() {
        let mut b = folded_sample();
        b.toggle_fold(1);
        b.click(0, 1, false); // 第 0 行末尾回车，下面所有行 +1
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
}
