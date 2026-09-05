//! 正则表达式测试 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `RegexView`：模式输入 + 五个标志开关 +
//! 待匹配文本（虚拟化编辑区）+ 匹配结果列表（含捕获分组）。
//! 匹配逻辑全在 `ferric_core::regex`（fancy-regex，支持前后瞻），没动过。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::regex;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct RegexDraft {
    pattern: String,
    flags: String,
    text: String,
}

/// 一条匹配结果，已摊平成可直接显示的形状。
pub struct MatchRow {
    /// `#1 · 12–24` 这种定位标签。
    pub label: String,
    pub text: String,
    /// 捕获分组，未参与匹配的显示为 `—`（而不是悄悄跳过 ——
    /// 分组编号必须与模式里的括号一一对应，否则用户会数错）。
    pub groups: Vec<String>,
}

pub struct RegexTool {
    pub pattern: String,
    pub fg: bool,
    pub fi: bool,
    pub fm: bool,
    pub fs: bool,
    pub fx: bool,
    pub text: TextBuffer,
    pub matches: Vec<MatchRow>,
    pub ok: bool,
    pub status: String,
}

impl Default for RegexTool {
    fn default() -> Self {
        let mut t = Self {
            pattern: r"(\w+)@(\w+\.\w+)".to_owned(),
            fg: true,
            fi: false,
            fm: false,
            fs: false,
            fx: false,
            text: TextBuffer::new(
                "联系我们：hi@ferric.dev 或 support@example.com，也可发送到 dev@ferric.io。",
            ),
            matches: Vec::new(),
            ok: true,
            status: String::new(),
        };
        t.run();
        t
    }
}

impl RegexTool {
    pub fn flags(&self) -> String {
        let mut s = String::new();
        for (on, c) in [
            (self.fg, 'g'),
            (self.fi, 'i'),
            (self.fm, 'm'),
            (self.fs, 's'),
            (self.fx, 'x'),
        ] {
            if on {
                s.push(c);
            }
        }
        s
    }

    /// 重新匹配。模式非法时**保留上一次的结果**并报错 —— 用户正在敲模式，
    /// 中间态（如刚打出一个 `(`）必然非法，清空列表会让下方一直在闪。
    pub fn run(&mut self) {
        match regex::find_all(&self.pattern, &self.flags(), &self.text.text()) {
            Ok(found) => {
                self.matches = found
                    .iter()
                    .enumerate()
                    .map(|(i, m)| MatchRow {
                        label: format!("#{} · {}–{}", i + 1, m.start, m.end),
                        text: m.text.clone(),
                        groups: m
                            .groups
                            .iter()
                            .map(|g| g.clone().unwrap_or_else(|| "—".to_owned()))
                            .collect(),
                    })
                    .collect();
                self.ok = true;
                self.status = if self.pattern.is_empty() {
                    "输入模式开始匹配".to_owned()
                } else {
                    format!("{} 处匹配", self.matches.len())
                };
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("模式非法：{e}");
            }
        }
    }

    /// 标志开关。索引对应 `g i m s x`。
    pub fn toggle_flag(&mut self, idx: i32) {
        match idx {
            0 => self.fg = !self.fg,
            1 => self.fi = !self.fi,
            2 => self.fm = !self.fm,
            3 => self.fs = !self.fs,
            4 => self.fx = !self.fx,
            _ => return,
        }
        self.run();
    }

    pub fn set_pattern(&mut self, p: &str) {
        self.pattern = p.to_owned();
        self.run();
    }
}

impl Tool for RegexTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "regex",
            name: "正则表达式",
            desc: "g/i/m/s/x 标志，分组捕获展示，支持前后瞻等 JS 常见语法。",
            icon: icons::TERMINAL,
            group: "文本",
            keywords: &["regex", "正则", "regexp", "match"],
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&RegexDraft {
            pattern: self.pattern.clone(),
            flags: self.flags(),
            text: self.text.text(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<RegexDraft>(data) {
            self.pattern = d.pattern;
            self.fg = d.flags.contains('g');
            self.fi = d.flags.contains('i');
            self.fm = d.flags.contains('m');
            self.fs = d.flags.contains('s');
            self.fx = d.flags.contains('x');
            self.text.set_text(&d.text);
            self.run();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_finds_all_three_addresses() {
        let t = RegexTool::default();
        assert!(t.ok, "{}", t.status);
        assert_eq!(t.matches.len(), 3);
        assert!(t.status.contains("3 处"));
    }

    #[test]
    fn groups_are_exposed_in_order() {
        let t = RegexTool::default();
        let first = &t.matches[0];
        assert_eq!(first.groups.len(), 2, "模式里有两个捕获分组");
        assert_eq!(first.groups[0], "hi");
        assert_eq!(first.groups[1], "ferric.dev");
    }

    #[test]
    fn non_participating_groups_render_as_dash_not_skipped() {
        // 分组编号必须与模式里的括号一一对应，跳过会让用户数错编号。
        let mut t = RegexTool::default();
        t.text.set_text("abc");
        t.set_pattern(r"(a)(x)?(b)");
        assert!(t.ok, "{}", t.status);
        assert_eq!(t.matches.len(), 1);
        assert_eq!(t.matches[0].groups, vec!["a", "—", "b"]);
    }

    #[test]
    fn without_g_only_the_first_match_is_returned() {
        // 与 JS 一致：不带 g 只返回首个匹配。
        let mut t = RegexTool::default();
        t.toggle_flag(0); // 关掉 g
        assert!(!t.fg);
        assert_eq!(t.matches.len(), 1);
    }

    #[test]
    fn case_insensitive_flag_takes_effect() {
        let mut t = RegexTool::default();
        t.text.set_text("HELLO hello");
        t.set_pattern("hello");
        let before = t.matches.len();
        t.toggle_flag(1); // 开 i
        assert!(t.fi);
        assert!(
            t.matches.len() > before,
            "开了 i 之后匹配数没变多：{} → {}",
            before,
            t.matches.len()
        );
    }

    #[test]
    fn invalid_pattern_keeps_the_last_good_result() {
        // 用户敲模式时必然经过非法中间态（例如刚打出一个左括号）。
        let mut t = RegexTool::default();
        let good = t.matches.len();
        assert!(good > 0);
        t.set_pattern("(unclosed");
        assert!(!t.ok, "非法模式必须报错");
        assert_eq!(t.matches.len(), good, "失败时不能把上次结果清掉");
        assert!(t.status.contains("非法"));
    }

    #[test]
    fn empty_pattern_is_not_an_error() {
        let mut t = RegexTool::default();
        t.set_pattern("");
        assert!(t.ok, "空模式不是错误，那是初始状态");
        assert!(t.matches.is_empty());
    }

    #[test]
    fn draft_roundtrip_preserves_pattern_flags_and_text() {
        let mut t = RegexTool::default();
        t.set_pattern(r"\d+");
        t.toggle_flag(1); // i
        t.toggle_flag(2); // m
        t.text.set_text("a1 b22");
        t.run();
        let saved = t.save_draft().expect("必须持久化草稿");

        let mut r = RegexTool::default();
        r.load_draft(&saved);
        assert_eq!(r.pattern, r"\d+");
        assert!(r.fi && r.fm, "标志没跟着草稿恢复：{}", r.flags());
        assert_eq!(r.text.text(), "a1 b22");
        assert_eq!(r.matches.len(), 2);
    }

    #[test]
    fn out_of_range_flag_index_is_ignored() {
        let mut t = RegexTool::default();
        let before = t.flags();
        t.toggle_flag(99);
        t.toggle_flag(-1);
        assert_eq!(t.flags(), before);
    }

    #[test]
    fn a_huge_subject_text_stays_virtualized() {
        let big: String = (0..9000).map(|i| format!("row {i} a{i}@x.com\n")).collect();
        let mut t = RegexTool::default();
        t.text.set_text(&big);
        t.text.set_viewport(26, 100);
        t.run();
        assert!(t.text.total_lines() > 2190);
        assert_eq!(t.text.visible_lines().len(), 26);
        assert_eq!(t.matches.len(), 9000);
    }
}
