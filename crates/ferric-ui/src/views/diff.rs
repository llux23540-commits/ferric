//! 文本 / 文件对比 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `DiffView`：左右两个虚拟化编辑区（输入）+
//! 下方统一的差异视图（结果）。比较逻辑在 `ferric_core::diff`（similar，
//! 带字符级片段），没动过。
//!
//! # 为什么结果用「统一视图」而不是左右并排高亮
//!
//! egui 版把差异直接高亮在左右两个编辑面板里，并做左右同步滚动。那需要
//! 「编辑区能按行着色」这个能力 —— 我们自己的 [`crate::editor::TextBuffer`]
//! 只渲染纯文本行，给它加逐行背景色会把「选区矩形 + 行背景 + 光标」三套
//! 坐标系统搅在一起。
//!
//! 统一视图（一行一行列出，带 `-`/`+` 与左右行号）反而更适合这里：
//! 差异行本来就要对照着看，而且行号成对显示比左右滚动条对齐更可靠。
//! 输入侧仍是两个独立的虚拟化编辑区，粘几万行不会崩。

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

/// 结果里的一行，已摊平成可直接显示的形状。
pub struct ResultRow {
    /// `-` / `+` / 空（相同行）。
    pub sign: &'static str,
    /// 左侧行号，插入行为空串。
    pub left_no: String,
    /// 右侧行号，删除行为空串。
    pub right_no: String,
    /// 行内片段：(文本, 是否为改动部分)。改动部分在界面上加深底色。
    pub segs: Vec<(String, bool)>,
    pub tag: Tag,
}

pub struct DiffTool {
    pub left: TextBuffer,
    pub right: TextBuffer,
    pub rows: Vec<ResultRow>,
    pub stats: DiffStats,
    /// 只看差异行（相同行折叠掉）。默认**关** —— 上下文对读懂差异是必要的。
    pub only_changes: bool,
    pub status: String,
}

impl Default for DiffTool {
    fn default() -> Self {
        let mut t = Self {
            left: TextBuffer::new("hello ferric\nline two\nsame tail\n"),
            right: TextBuffer::new("hello slint\nline two\nnew line\nsame tail\n"),
            rows: Vec::new(),
            stats: DiffStats::default(),
            only_changes: false,
            status: String::new(),
        };
        t.compare();
        t
    }
}

impl DiffTool {
    pub fn compare(&mut self) {
        let (lines, stats) = diff::line_diff(&self.left.text(), &self.right.text());
        self.stats = stats;
        self.rows = lines
            .into_iter()
            .filter(|l| !self.only_changes || l.tag != Tag::Equal)
            .map(|l| ResultRow {
                sign: match l.tag {
                    Tag::Delete => "-",
                    Tag::Insert => "+",
                    Tag::Equal => " ",
                },
                left_no: l.left_no.map(|n| n.to_string()).unwrap_or_default(),
                right_no: l.right_no.map(|n| n.to_string()).unwrap_or_default(),
                segs: l.segs.into_iter().map(|s| (s.text, s.emph)).collect(),
                tag: l.tag,
            })
            .collect();
        self.status = format!(
            "+{} 新增 · -{} 删除 · {} 行相同",
            stats.added, stats.removed, stats.unchanged
        );
    }

    pub fn toggle_only_changes(&mut self) {
        self.only_changes = !self.only_changes;
        self.compare();
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
            desc: "逐行 diff，改动行带字符级高亮，可只看差异行，可左右互换。",
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
    fn changed_lines_carry_character_level_segments() {
        // 字符级高亮是这个工具的核心价值：一眼看出「改了哪几个字」。
        let mut t = DiffTool::default();
        t.left.set_text("hello world\n");
        t.right.set_text("hello WORLD\n");
        t.compare();
        let emphasized: usize = t
            .rows
            .iter()
            .flat_map(|r| r.segs.iter())
            .filter(|(_, emph)| *emph)
            .count();
        assert!(emphasized > 0, "改动行没有字符级片段");
    }

    #[test]
    fn line_numbers_are_paired_and_omitted_where_absent() {
        // 插入行没有左侧行号，删除行没有右侧行号。填成 0 或复用上一行的号
        // 都会让人对错行。
        let mut t = DiffTool::default();
        t.left.set_text("a\n");
        t.right.set_text("a\nb\n");
        t.compare();
        let inserted = t
            .rows
            .iter()
            .find(|r| r.tag == Tag::Insert)
            .expect("应当有一行插入");
        assert!(inserted.left_no.is_empty(), "插入行不该有左侧行号");
        assert!(!inserted.right_no.is_empty());
    }

    #[test]
    fn only_changes_filters_out_equal_lines() {
        let mut t = DiffTool::default();
        let all = t.rows.len();
        assert!(t.rows.iter().any(|r| r.tag == Tag::Equal), "前提：有相同行");
        t.toggle_only_changes();
        assert!(t.only_changes);
        assert!(t.rows.len() < all);
        assert!(
            t.rows.iter().all(|r| r.tag != Tag::Equal),
            "开了「只看差异」还留着相同行"
        );
        // 统计数字**不受过滤影响** —— 那是整份文档的事实
        assert_eq!(t.stats.unchanged, DiffTool::default().stats.unchanged);
    }

    #[test]
    fn only_changes_defaults_to_off() {
        // 上下文对读懂差异是必要的。
        assert!(!DiffTool::default().only_changes);
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
        let mut t = DiffTool::default();
        t.toggle_only_changes();
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
