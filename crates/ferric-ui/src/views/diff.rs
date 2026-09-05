//! 文本 / 文件对比 —— 差异**画在左右原文上**（git 的做法）。
//!
//! 视图在 `ui/app.slint` 的 `DiffView`：左右两个虚拟化编辑区，删除行整行偏红、
//! 新增行整行偏绿、改动的那几个字加深底色，行号槽里跟一个 `-` / `+`。
//! 比较逻辑在 `ferric_core::diff`（similar，带字符级片段），没动过。
//!
//! # 为什么不是「下面单独列一份统一视图」
//!
//! 迁 Slint 时先做成了「上面两个输入框 + 下面一份统一 diff」。那份统一视图有
//! 两个真问题：**差异不在原文的位置上**（要在两个地方之间来回找同一行），
//! 以及它把每一行都实例化进 `ScrollView` —— 输入侧虚拟化了，结果侧没有，
//! 几万行的对比照样会把布局撑爆。
//!
//! 现在逐行种类与字符级高亮由这里算好（下标 = 文档行号），编辑区按可见行取用，
//! 所以「几十万行也只渲染一屏」这个前提对差异高亮同样成立。
//!
//! # 对齐
//!
//! 不插空行做严格对齐（那要在编辑区里造出「不存在的行」，光标与选区坐标全得
//! 跟着分叉）。取而代之：**左右同步滚动按 diff 行对位** —— 滚哪一侧，另一侧
//! 跳到同一处 hunk；外加「上一处 / 下一处差异」两个按钮直接跳。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::diff::{self, DiffStats, Tag};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct DiffDraft {
    left: String,
    right: String,
}

/// 导出用的一行（「复制结果」把差异导成 unified diff 文本）。
pub struct ResultRow {
    /// `-` / `+` / 空（相同行）。
    pub sign: &'static str,
    /// 行内片段：(文本, 是否为改动部分)。
    pub segs: Vec<(String, bool)>,
    pub tag: Tag,
}

/// 逐行差异种类。下标 = 文档行号，值进 `EditorState.row-kinds`。
pub const KIND_SAME: i32 = 0;
pub const KIND_DEL: i32 = 1;
pub const KIND_ADD: i32 = 2;

pub struct DiffTool {
    pub left: TextBuffer,
    pub right: TextBuffer,
    pub rows: Vec<ResultRow>,
    pub stats: DiffStats,
    pub status: String,
    /// 每一行的差异种类，下标 = 文档行号（左侧只会有删除，右侧只会有新增）。
    pub left_kinds: Vec<i32>,
    pub right_kinds: Vec<i32>,
    /// 字符级高亮 `(行, 起始 char, 长度)` —— 「改了哪几个字」。
    pub left_emph: Vec<(usize, usize, usize)>,
    pub right_emph: Vec<(usize, usize, usize)>,
    /// diff 行 → `(左行, 右行, 种类)`（行号 0 起）。左右同步滚动与跳 hunk 用；
    /// 插入行没有左行、删除行没有右行，所以两侧都是 `Option`。
    align: Vec<(Option<usize>, Option<usize>, Tag)>,
    /// 当前停在第几处 hunk（`hunk_starts()` 的下标）。比较一次就清空。
    hunk_at: Option<usize>,
}

impl Default for DiffTool {
    fn default() -> Self {
        let mut t = Self {
            left: TextBuffer::new("hello ferric\nline two\nsame tail\n"),
            right: TextBuffer::new("hello slint\nline two\nnew line\nsame tail\n"),
            rows: Vec::new(),
            stats: DiffStats::default(),
            status: String::new(),
            left_kinds: Vec::new(),
            right_kinds: Vec::new(),
            left_emph: Vec::new(),
            right_emph: Vec::new(),
            align: Vec::new(),
            hunk_at: None,
        };
        t.compare();
        t
    }
}

impl DiffTool {
    pub fn compare(&mut self) {
        let (lines, stats) = diff::line_diff(&self.left.text(), &self.right.text());
        self.stats = stats;

        self.left_kinds = vec![KIND_SAME; self.left.total_lines()];
        self.right_kinds = vec![KIND_SAME; self.right.total_lines()];
        self.left_emph.clear();
        self.right_emph.clear();
        self.align.clear();
        // 行号全变了，「停在第几处」也就没意义了。
        self.hunk_at = None;
        self.rows.clear();

        for l in lines {
            let (lno, rno) = (l.left_no.map(|n| n - 1), l.right_no.map(|n| n - 1));
            self.align.push((lno, rno, l.tag));

            // 行内高亮：按 char 偏移记 `(行, 起, 长)` —— 中文一个字是一个 char，
            // 按字节记会在编辑区的列坐标上错位。
            let (kinds, emph, no) = match l.tag {
                Tag::Delete => (&mut self.left_kinds, &mut self.left_emph, lno),
                Tag::Insert => (&mut self.right_kinds, &mut self.right_emph, rno),
                Tag::Equal => (&mut self.left_kinds, &mut self.left_emph, None),
            };
            if let Some(line) = no {
                if let Some(k) = kinds.get_mut(line) {
                    *k = if l.tag == Tag::Delete {
                        KIND_DEL
                    } else {
                        KIND_ADD
                    };
                }
                let mut col = 0usize;
                for s in &l.segs {
                    let n = s.text.chars().count();
                    if s.emph && n > 0 {
                        emph.push((line, col, n));
                    }
                    col += n;
                }
            }

            self.rows.push(ResultRow {
                sign: match l.tag {
                    Tag::Delete => "-",
                    Tag::Insert => "+",
                    Tag::Equal => " ",
                },
                segs: l.segs.into_iter().map(|s| (s.text, s.emph)).collect(),
                tag: l.tag,
            });
        }

        self.status = format!(
            "+{} 新增 · -{} 删除 · {} 行相同",
            stats.added, stats.removed, stats.unchanged
        );
    }

    /// 左右同步滚动：把另一侧滚到「同一处 diff 行」。
    ///
    /// 不插空行做严格对齐（见模块头），所以对位发生在滚动这一刻：
    /// 滚动侧视口顶端那一行落在第几个 diff 行，另一侧就滚到同一个 diff 行。
    pub fn mirror_scroll(&mut self, from_left: bool) {
        let src = if from_left {
            self.left.scroll_line()
        } else {
            self.right.scroll_line()
        };
        let row = self.row_of(from_left, src);
        let Some(dst) = self.line_at(!from_left, row) else {
            return;
        };
        if from_left {
            self.right.scroll_to_doc_line(dst);
        } else {
            self.left.scroll_to_doc_line(dst);
        }
    }

    /// 跳到下一处 / 上一处差异（两侧一起动），到头循环。没有差异时不动视野。
    pub fn next_hunk(&mut self) {
        self.jump_hunk(true);
    }

    pub fn prev_hunk(&mut self) {
        self.jump_hunk(false);
    }

    /// 「第几处 hunk」是自己记的，不从滚动位置反推。
    ///
    /// 反推过一版：跳过去之后要留两行上下文，视口顶端因此落在 hunk 之前，
    /// 下一次反推又算回同一处 —— 按钮点第二下没有反应。
    fn jump_hunk(&mut self, forward: bool) {
        let starts = self.hunk_starts();
        if starts.is_empty() {
            return;
        }
        let i = match self.hunk_at {
            None => {
                if forward {
                    0
                } else {
                    starts.len() - 1
                }
            }
            Some(i) if forward => (i + 1) % starts.len(),
            Some(i) => (i + starts.len() - 1) % starts.len(),
        };
        self.hunk_at = Some(i);

        // 上面留两行上下文：hunk 贴着视口顶边很难看出改的是什么。
        let top = starts[i].saturating_sub(2);
        if let Some(l) = self.line_at(true, top) {
            self.left.scroll_to_doc_line(l);
        }
        if let Some(r) = self.line_at(false, top) {
            self.right.scroll_to_doc_line(r);
        }
    }

    /// 每一处 hunk 的起始 diff 行（连续的差异行算一处）。
    fn hunk_starts(&self) -> Vec<usize> {
        self.align
            .iter()
            .enumerate()
            .filter(|(i, (_, _, tag))| {
                *tag != Tag::Equal && (*i == 0 || self.align[i - 1].2 == Tag::Equal)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// 某一侧的某一行落在第几个 diff 行。
    fn row_of(&self, left: bool, line: usize) -> usize {
        self.align
            .iter()
            .position(|(l, r, _)| {
                let side = if left { l } else { r };
                side.is_some_and(|n| n >= line)
            })
            .unwrap_or(self.align.len().saturating_sub(1))
    }

    /// 从第 `row` 个 diff 行往后找，某一侧的第一个真实行号。
    fn line_at(&self, left: bool, row: usize) -> Option<usize> {
        self.align
            .iter()
            .skip(row)
            .find_map(|(l, r, _)| if left { *l } else { *r })
    }

    /// 左右互换。对比方向搞反是很常见的事，换一下比重新粘两遍快。
    pub fn swap(&mut self) {
        let l = self.left.text();
        let r = self.right.text();
        self.left.set_text(&r);
        self.right.set_text(&l);
        self.compare();
    }

    pub fn clear(&mut self) {
        self.left.set_text("");
        self.right.set_text("");
        self.compare();
    }

    /// 把差异结果导成 unified diff 风格的纯文本（供复制）。
    pub fn as_text(&self) -> String {
        let mut out = String::new();
        for r in &self.rows {
            out.push_str(r.sign);
            out.push(' ');
            for (t, _) in &r.segs {
                out.push_str(t);
            }
            out.push('\n');
        }
        out
    }
}

impl Tool for DiffTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "diff",
            name: "文本 / 文件对比",
            desc: "逐行 diff，差异直接画在左右原文上（改动的字加深底色），可跳到下一处差异、可左右互换。",
            icon: icons::GIT_COMPARE,
            group: "对比",
            keywords: &["diff", "compare", "对比", "比较", "差异"],
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&DiffDraft {
            left: self.left.text(),
            right: self.right.text(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<DiffDraft>(data) {
            self.left.set_text(&d.left);
            self.right.set_text(&d.right);
            self.compare();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_reports_one_change_and_one_insert() {
        let t = DiffTool::default();
        assert_eq!(t.stats.added, 2, "hello slint 与 new line 两行新增");
        assert_eq!(t.stats.removed, 1, "hello ferric 一行删除");
        assert!(t.status.contains("新增"));
    }

    #[test]
    fn identical_input_has_no_changes() {
        let mut t = DiffTool::default();
        t.left.set_text("same\ntext\n");
        t.right.set_text("same\ntext\n");
        t.compare();
        assert_eq!(t.stats.added, 0);
        assert_eq!(t.stats.removed, 0);
        assert!(t.rows.iter().all(|r| r.tag == Tag::Equal));
    }

    #[test]
    fn each_side_is_marked_with_its_own_kind() {
        // 这是「差异画在原文上」的全部内容：左边只会红（删除），
        // 右边只会绿（新增），相同行两边都不着色。
        let mut t = DiffTool::default();
        t.left.set_text("a\nb\nc\n");
        t.right.set_text("a\nB\nc\n");
        t.compare();

        assert_eq!(t.left_kinds[0], KIND_SAME);
        assert_eq!(t.left_kinds[1], KIND_DEL, "左侧第二行被删掉了");
        assert_eq!(t.left_kinds[2], KIND_SAME);

        assert_eq!(t.right_kinds[0], KIND_SAME);
        assert_eq!(t.right_kinds[1], KIND_ADD, "右侧第二行是新增");
        assert_eq!(t.right_kinds[2], KIND_SAME);
        assert!(
            t.left_kinds.iter().all(|k| *k != KIND_ADD),
            "左侧不该出现新增色"
        );
        assert!(
            t.right_kinds.iter().all(|k| *k != KIND_DEL),
            "右侧不该出现删除色"
        );
    }

    #[test]
    fn changed_words_are_marked_by_char_offset() {
        // 字符级高亮是这个工具的核心价值：一眼看出「改了哪几个字」。
        // 偏移必须按 char 记 —— 按字节记会在中文上错位。
        let mut t = DiffTool::default();
        t.left.set_text("你好 世界\n");
        t.right.set_text("你好 银河\n");
        t.compare();

        assert!(!t.left_emph.is_empty(), "删除侧没有字符级高亮");
        assert!(!t.right_emph.is_empty(), "新增侧没有字符级高亮");
        for (line, col, len) in t.left_emph.iter().chain(t.right_emph.iter()) {
            assert_eq!(*line, 0);
            // 「你好 」三个 char 之后才是被改的部分
            assert!(*col >= 3, "高亮起点按 char 算应当 ≥ 3，实际 {col}");
            assert!(*col + *len <= 5, "高亮不该越过行尾（5 个 char）");
        }
    }

    #[test]
    fn kinds_cover_every_line_of_both_documents() {
        // 编辑区按文档行号索引这两个数组，短一格就会在最后一行 panic 或漏色。
        let mut t = DiffTool::default();
        t.left.set_text("1\n2\n3\n4\n");
        t.right.set_text("1\n");
        t.compare();
        assert_eq!(t.left_kinds.len(), t.left.total_lines());
        assert_eq!(t.right_kinds.len(), t.right.total_lines());
    }

    #[test]
    fn scrolling_one_side_lines_up_the_other() {
        // 不插空行做严格对齐，对位发生在滚动这一刻。
        let left: String = (0..60).map(|i| format!("line {i}\n")).collect();
        // 右侧在开头多插 5 行 —— 之后同一句话在两侧的行号差 5。
        let right = format!("x\nx\nx\nx\nx\n{left}");
        let mut t = DiffTool::default();
        t.left.set_text(&left);
        t.right.set_text(&right);
        t.left.set_viewport(20, 80);
        t.right.set_viewport(20, 80);
        t.compare();

        t.left.scroll_to_doc_line(30);
        t.mirror_scroll(true);
        assert_eq!(
            t.right.scroll_line(),
            35,
            "右侧要停在同一句话上（左 30 = 右 35）"
        );

        t.right.scroll_to_doc_line(40);
        t.mirror_scroll(false);
        assert_eq!(t.left.scroll_line(), 35, "反方向同样对位");
    }

    #[test]
    fn hunk_navigation_walks_the_changes() {
        let mut t = DiffTool::default();
        t.left
            .set_text("same\nsame\nold\nsame\nsame\nsame\nold2\nsame\n");
        t.right
            .set_text("same\nsame\nnew\nsame\nsame\nsame\nnew2\nsame\n");
        t.left.set_viewport(4, 80);
        t.right.set_viewport(4, 80);
        t.compare();

        t.left.scroll_to_doc_line(0);
        t.next_hunk();
        let first = t.left.scroll_line();
        t.next_hunk();
        let second = t.left.scroll_line();
        assert!(second > first, "第二处差异应当更靠后（{first} → {second}）");

        t.prev_hunk();
        assert_eq!(t.left.scroll_line(), first, "往回跳应当回到上一处");
    }

    #[test]
    fn hunk_navigation_is_a_noop_without_differences() {
        let mut t = DiffTool::default();
        t.left.set_text("a\nb\n");
        t.right.set_text("a\nb\n");
        t.compare();
        t.left.scroll_to_doc_line(1);
        t.next_hunk();
        assert_eq!(t.left.scroll_line(), 1, "没有差异就别乱跳视野");
    }

    #[test]
    fn swap_exchanges_both_sides_and_flips_the_counts() {
        let mut t = DiffTool::default();
        let (added, removed) = (t.stats.added, t.stats.removed);
        let (l, r) = (t.left.text(), t.right.text());
        t.swap();
        assert_eq!(t.left.text(), r);
        assert_eq!(t.right.text(), l);
        assert_eq!(t.stats.added, removed, "互换后新增/删除应当对调");
        assert_eq!(t.stats.removed, added);
    }

    #[test]
    fn export_text_is_unified_diff_shaped() {
        // 「复制结果」导出的仍是统一 diff 文本 —— 界面上不再单列一份，
        // 但贴进工单 / 聊天窗口的时候需要它。
        let t = DiffTool::default();
        let text = t.as_text();
        assert!(
            text.lines().any(|l| l.starts_with('-')),
            "缺少删除行：{text}"
        );
        assert!(
            text.lines().any(|l| l.starts_with('+')),
            "缺少新增行：{text}"
        );
    }

    #[test]
    fn draft_roundtrip_preserves_both_sides() {
        let mut t = DiffTool::default();
        t.left.set_text("L\n");
        t.right.set_text("R\n");
        t.compare();
        let saved = t.save_draft().expect("必须持久化草稿");

        let mut r = DiffTool::default();
        r.load_draft(&saved);
        assert_eq!(r.left.text(), "L\n");
        assert_eq!(r.right.text(), "R\n");
        assert!(!r.rows.is_empty(), "恢复草稿后应当已经比较过");
    }

    #[test]
    fn both_sides_stay_virtualized_on_huge_input() {
        // 崩溃守门：Slint 原生 TextEdit 在约 2190 行以上 panic。
        let a: String = (0..9000).map(|i| format!("line {i}\n")).collect();
        let b: String = (0..9000).map(|i| format!("line {}\n", i * 2)).collect();
        let mut t = DiffTool::default();
        t.left.set_text(&a);
        t.right.set_text(&b);
        t.left.set_viewport(24, 100);
        t.right.set_viewport(24, 100);
        t.compare();
        assert!(t.left.total_lines() > 2190);
        assert_eq!(t.left.visible_lines().len(), 24);
        assert_eq!(t.right.visible_lines().len(), 24);
    }

    #[test]
    fn empty_input_does_not_panic() {
        let mut t = DiffTool::default();
        t.clear();
        assert_eq!(t.stats.added, 0);
        assert_eq!(t.stats.removed, 0);
    }
}
