//! 逐行文本对比。

use similar::{ChangeTag, TextDiff};

/// 单行差异标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Equal,
    Delete,
    Insert,
}

/// 行内片段（用于字符级高亮）。`emph=true` 表示该片段是被改动的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub emph: bool,
}

/// 一行对比结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub tag: Tag,
    /// 左侧行号（1 起）；插入行为 None。
    pub left_no: Option<usize>,
    /// 右侧行号（1 起）；删除行为 None。
    pub right_no: Option<usize>,
    /// 行内片段（拼起来即整行文本），改动行含字符级高亮。
    pub segs: Vec<Seg>,
}

impl DiffLine {
    /// 整行纯文本。
    pub fn text(&self) -> String {
        self.segs.iter().map(|s| s.text.as_str()).collect()
    }
}

/// 统计摘要。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
    pub unchanged: usize,
}

/// 逐行比较两段文本，改动行带字符级片段。
pub fn line_diff(left: &str, right: &str) -> (Vec<DiffLine>, DiffStats) {
    let diff = TextDiff::from_lines(left, right);
    let mut lines = Vec::new();
    let mut stats = DiffStats::default();
    let mut lno = 0usize;
    let mut rno = 0usize;

    for op in diff.ops() {
        for change in diff.iter_inline_changes(op) {
            // 收集行内片段并去掉行尾换行。
            let mut segs: Vec<Seg> = change
                .iter_strings_lossy()
                .map(|(emph, s)| Seg {
                    text: s.into_owned(),
                    emph,
                })
                .collect();
            if let Some(last) = segs.last_mut() {
                let t = last.text.trim_end_matches('\n');
                if t.len() != last.text.len() {
                    last.text = t.to_string();
                }
            }
            segs.retain(|s| !s.text.is_empty());

            match change.tag() {
                ChangeTag::Equal => {
                    lno += 1;
                    rno += 1;
                    stats.unchanged += 1;
                    lines.push(DiffLine {
                        tag: Tag::Equal,
                        left_no: Some(lno),
                        right_no: Some(rno),
                        segs,
                    });
                }
                ChangeTag::Delete => {
                    lno += 1;
                    stats.removed += 1;
                    lines.push(DiffLine {
                        tag: Tag::Delete,
                        left_no: Some(lno),
                        right_no: None,
                        segs,
                    });
                }
                ChangeTag::Insert => {
                    rno += 1;
                    stats.added += 1;
                    lines.push(DiffLine {
                        tag: Tag::Insert,
                        left_no: None,
                        right_no: Some(rno),
                        segs,
                    });
                }
            }
        }
    }
    (lines, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_added_and_removed() {
        let (lines, stats) = line_diff("a\nb\nc\n", "a\nc\nd\n");
        assert_eq!(stats.removed, 1); // b
        assert_eq!(stats.added, 1); // d
        assert_eq!(stats.unchanged, 2); // a, c
        assert!(lines
            .iter()
            .any(|l| l.tag == Tag::Delete && l.text() == "b"));
        assert!(lines
            .iter()
            .any(|l| l.tag == Tag::Insert && l.text() == "d"));
    }

    #[test]
    fn inline_emphasis_on_changed_line() {
        // "foo1" -> "foo2"：应有强调片段
        let (lines, _) = line_diff("foo1\n", "foo2\n");
        let any_emph = lines
            .iter()
            .filter(|l| l.tag != Tag::Equal)
            .flat_map(|l| &l.segs)
            .any(|s| s.emph);
        assert!(any_emph);
    }

    #[test]
    fn identical_has_no_changes() {
        let (_, stats) = line_diff("x\ny\n", "x\ny\n");
        assert_eq!(stats.added, 0);
        assert_eq!(stats.removed, 0);
    }
}
