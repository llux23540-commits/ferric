//! JSON 工具 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `JsonView`：工具条（格式化 / 压缩 / 校验 / 转义 /
//! 去转义 / 排序 / 缩进档位）+ 虚拟化编辑区 + 搜索条。
//! 解析与变换全在 `ferric_core::json`（serde_json，`preserve_order`），没动过。
//!
//! # 与 egui 版的关键差别
//!
//! egui 版必须每帧比一次 `input` 与 `baseline` 的差异才能发现「用户手动改了
//! 文本」—— 因为编辑器直接就地改 `&mut Rope`，不通知外部。它还得为此维护
//! `validated` 快照来避免每帧重新解析 5MB JSON（`Value` 树能到 ~67MB）。
//!
//! Slint 这边编辑走的是 [`crate::editor_bridge::apply_key`]，**改动是显式事件**，
//! 外壳在按键后调一次 [`JsonTool::on_edited`] 即可。那两套「每帧比差异」的
//! 补偿逻辑整体不需要了。撤销重做也直接落在 [`TextBuffer`] 里（带字节限流）。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::json::{self, Indent};
use serde::{Deserialize, Serialize};

/// 缩进档位（对应界面上的分段控件）。
pub const INDENTS: [Indent; 3] = [Indent::Two, Indent::Four, Indent::Tab];

fn default_wrap() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct JsonDraft {
    input: String,
    indent: Indent,
    sort: bool,
    /// 老草稿没有这个字段，缺省按开启处理（与新装用户一致）。
    #[serde(default = "default_wrap")]
    wrap: bool,
}

pub struct JsonTool {
    pub input: TextBuffer,
    pub indent: Indent,
    pub sort: bool,
    /// 自动换行。默认开 —— 格式化后的长行（长 URL、base64、压缩过的单行 JSON）
    /// 一旦超出可视宽度，不换行就只能靠横向滚动一点点找，实际是看不见。
    pub wrap: bool,
    pub ok: bool,
    pub status: String,
    /// 开启键名排序前的那份正文：关掉排序时据此**退回原始键序**。
    /// 只存运行时 —— 重启后原始键序已不可考，退回当前正文即可。
    unsorted: Option<String>,

    // ——— 搜索 ———
    pub find: String,
    /// 命中位置（字符索引）。搜索是显式动作，不随打字实时重算 ——
    /// 5MB 文本上每个字符都全量搜一遍是白烧。
    pub hits: Vec<usize>,
    pub hit_idx: usize,
}

impl Default for JsonTool {
    fn default() -> Self {
        let mut t = Self {
            input: TextBuffer::new("{\n  \"hello\": \"ferric\",\n  \"items\": [1, 2, 3]\n}"),
            indent: Indent::Two,
            sort: false,
            wrap: true,
            ok: true,
            status: String::new(),
            unsorted: None,
            find: String::new(),
            hits: Vec::new(),
            hit_idx: 0,
        };
        t.validate();
        t
    }
}

impl JsonTool {
    pub fn indent_index(&self) -> i32 {
        INDENTS.iter().position(|i| *i == self.indent).unwrap_or(0) as i32
    }

    pub fn set_indent_index(&mut self, i: i32) {
        if let Some(v) = INDENTS.get(i.max(0) as usize) {
            self.indent = *v;
            self.status = "缩进已改 —— 点「格式化」生效".to_owned();
        }
    }

    /// 实时校验。只更新状态，**不动正文** —— 用户正在打字。
    pub fn validate(&mut self) {
        let text = self.input.text();
        if text.trim().is_empty() {
            self.ok = true;
            self.status = "就绪".to_owned();
            return;
        }
        match json::validate(&text) {
            Ok(()) => {
                self.ok = true;
                self.status = format!("JSON 有效 · {} 行", self.input.total_lines());
            }
            Err(e) => {
                self.ok = false;
                self.status = e;
            }
        }
    }

    /// 文本被编辑后的钩子。外壳在按键处理后调一次。
    pub fn on_edited(&mut self) {
        // 正文变了，之前算好的命中位置全部作废 —— 留着会跳到错误的位置。
        self.hits.clear();
        self.hit_idx = 0;
        self.validate();
    }

    pub fn format(&mut self) {
        let text = self.input.text();
        match json::format(&text, self.indent, self.sort) {
            Ok(out) => {
                // 原地格式化：保留视野与光标行列，用户在第 500 行按格式化不该跳回顶部。
                self.input.replace_keeping_view(&out);
                self.ok = true;
                self.status = format!("已格式化 · {} 行", self.input.total_lines());
                self.hits.clear();
            }
            Err(e) => {
                self.ok = false;
                self.status = e;
            }
        }
    }

    pub fn minify(&mut self) {
        match json::minify(&self.input.text()) {
            Ok(out) => {
                self.input.replace_keeping_view(&out);
                self.ok = true;
                self.status = format!("已压缩 · {} 字节", self.input.len_bytes());
                self.hits.clear();
            }
            Err(e) => {
                self.ok = false;
                self.status = e;
            }
        }
    }

    /// 转义成 JSON 字符串字面量（把当前正文整体变成一个字符串）。
    pub fn escape(&mut self) {
        let out = json::escape(&self.input.text());
        self.input.replace_keeping_view(&out);
        self.ok = true;
        self.status = "已转义".to_owned();
        self.hits.clear();
    }

    /// 去转义。多层嵌套与内嵌 JSON 字符串一次剥完（`unescape_deep`）——
    /// 日志里捞出来的 JSON 常常被转义了两三层，一层层点是最烦人的操作之一。
    pub fn unescape(&mut self) {
        match json::unescape_deep(&self.input.text()) {
            Ok(out) => {
                self.input.replace_keeping_view(&out);
                self.ok = true;
                self.status = "已去转义（多层一次剥完）".to_owned();
                self.hits.clear();
            }
            Err(e) => {
                self.ok = false;
                self.status = e;
            }
        }
    }

    /// 切换键名排序。
    ///
    /// 关掉时**退回开启前的原始键序**，而不是把当前内容再排一遍 ——
    /// 后者做不到「撤销排序」，而那正是用户点这个开关想要的。
    pub fn toggle_sort(&mut self) {
        if !self.sort {
            self.unsorted = Some(self.input.text());
            self.sort = true;
            match json::format(&self.input.text(), self.indent, true) {
                Ok(out) => {
                    self.input.replace_keeping_view(&out);
                    self.ok = true;
                    self.status = "键名已排序".to_owned();
                }
                Err(e) => {
                    // 排不动就把开关退回去，别让界面显示「已排序」而内容没变。
                    self.sort = false;
                    self.unsorted = None;
                    self.ok = false;
                    self.status = e;
                }
            }
        } else {
            self.sort = false;
            if let Some(orig) = self.unsorted.take() {
                self.input.replace_keeping_view(&orig);
                self.status = "已恢复原始键序".to_owned();
                self.ok = true;
            } else {
                self.status = "已关闭键名排序".to_owned();
            }
        }
        self.hits.clear();
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.status = if self.wrap {
            "自动换行：开".to_owned()
        } else {
            "自动换行：关（横向滚动）".to_owned()
        };
    }

    pub fn clear(&mut self) {
        self.input.set_text("");
        self.hits.clear();
        self.hit_idx = 0;
        self.validate();
    }

    // ——— 搜索 ———

    /// 执行搜索（显式动作：点「查找」或按 Enter）。
    ///
    /// 不做「每敲一个字就全量搜」：5MB 文本上那是每次按键一次全文扫描。
    pub fn search(&mut self) {
        self.hits.clear();
        self.hit_idx = 0;
        let needle = self.find.clone();
        if needle.is_empty() {
            self.status = "输入要查找的内容".to_owned();
            return;
        }
        let text = self.input.text();
        // 按 char 索引记录命中位置 —— TextBuffer 的坐标是 char，不是字节。
        // 用字节位置会在中文文本上错位。
        let mut char_pos = 0usize;
        let mut rest = text.as_str();
        while let Some(byte_off) = rest.find(&needle) {
            char_pos += rest[..byte_off].chars().count();
            self.hits.push(char_pos);
            let step = needle.chars().count().max(1);
            char_pos += step;
            let advance = byte_off + needle.len();
            rest = &rest[advance..];
        }
        if self.hits.is_empty() {
            self.status = format!("未找到「{needle}」");
        } else {
            self.status = format!("{} 处命中（1/{}）", self.hits.len(), self.hits.len());
            self.jump_to_hit(0);
        }
    }

    /// 跳到第 n 个命中（循环）。
    pub fn next_hit(&mut self) {
        if self.hits.is_empty() {
            self.search();
            return;
        }
        let n = (self.hit_idx + 1) % self.hits.len();
        self.jump_to_hit(n);
    }

    pub fn prev_hit(&mut self) {
        if self.hits.is_empty() {
            self.search();
            return;
        }
        let n = if self.hit_idx == 0 {
            self.hits.len() - 1
        } else {
            self.hit_idx - 1
        };
        self.jump_to_hit(n);
    }

    fn jump_to_hit(&mut self, n: usize) {
        let Some(pos) = self.hits.get(n).copied() else {
            return;
        };
        self.hit_idx = n;
        // 选中命中的那一段，并把它滚进视野。
        let len = self.find.chars().count();
        self.input.select_range(pos, pos + len);
        self.status = format!(
            "{} 处命中（{}/{}）",
            self.hits.len(),
            n + 1,
            self.hits.len()
        );
    }

    pub fn set_find(&mut self, v: &str) {
        self.find = v.to_owned();
        // 关键词变了，旧命中作废（但不立刻重搜 —— 见 `search` 的说明）。
        self.hits.clear();
        self.hit_idx = 0;
    }

    pub fn can_undo(&self) -> bool {
        self.input.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.input.can_redo()
    }

    pub fn undo(&mut self) {
        self.input.undo();
        self.on_edited();
    }

    pub fn redo(&mut self) {
        self.input.redo();
        self.on_edited();
    }
}

impl Tool for JsonTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "json",
            name: "JSON 工具",
            desc:
                "格式化 / 压缩 / 校验 / 转义 / 去转义（多层一次剥完）/ 键名排序，查找与撤销重做。",
            icon: icons::BRACES,
            group: "JSON",
            keywords: &[
                "json",
                "format",
                "beautify",
                "minify",
                "美化",
                "格式化",
                "压缩",
            ],
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&JsonDraft {
            input: self.input.text(),
            indent: self.indent,
            sort: self.sort,
            wrap: self.wrap,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<JsonDraft>(data) {
            self.input.set_text(&d.input);
            self.indent = d.indent;
            self.sort = d.sort;
            self.wrap = d.wrap;
            self.validate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_validates_and_formats() {
        let mut t = JsonTool::default();
        assert!(t.ok, "{}", t.status);
        t.format();
        assert!(t.ok, "{}", t.status);
        assert!(t.input.text().contains("\"hello\""));
    }

    #[test]
    fn invalid_json_reports_but_keeps_the_text() {
        // 用户正在打字，中间态必然非法 —— 绝不能动他的正文。
        let mut t = JsonTool::default();
        t.input.set_text("{\"broken\": ");
        t.on_edited();
        assert!(!t.ok);
        assert_eq!(t.input.text(), "{\"broken\": ", "校验失败时改了正文");
    }

    #[test]
    fn minify_then_format_round_trips() {
        let mut t = JsonTool::default();
        t.format();
        let pretty = t.input.text();
        t.minify();
        assert_eq!(t.input.total_lines(), 1, "压缩后应当是单行");
        t.format();
        assert_eq!(t.input.text(), pretty, "格式化应当还原");
    }

    #[test]
    fn indent_option_takes_effect_on_next_format() {
        let mut t = JsonTool::default();
        t.set_indent_index(1); // 四空格
        t.format();
        assert!(
            t.input.text().contains("\n    \"hello\""),
            "四空格缩进没生效：{}",
            t.input.text()
        );
        t.set_indent_index(2); // Tab
        t.format();
        assert!(
            t.input.text().contains("\n\t\"hello\""),
            "Tab 缩进没生效：{}",
            t.input.text()
        );
    }

    #[test]
    fn escape_then_unescape_round_trips() {
        let mut t = JsonTool::default();
        t.format();
        let orig = t.input.text();
        t.escape();
        assert!(t.input.text().starts_with('"'), "转义后应当是字符串字面量");
        t.unescape();
        assert_eq!(t.input.text(), orig);
    }

    #[test]
    fn unescape_peels_multiple_layers_at_once() {
        // 日志里捞出来的 JSON 常被转义两三层，一层层点是最烦人的操作之一。
        let mut t = JsonTool::default();
        t.input.set_text(r#""{\"a\":\"{\\\"b\\\":1}\"}""#);
        t.unescape();
        assert!(t.ok, "{}", t.status);
        let out = t.input.text();
        assert!(!out.contains(r#"\""#), "还剩转义层没剥完：{out}");
    }

    #[test]
    fn turning_sort_off_restores_the_original_key_order() {
        // 这是这个开关的全部意义：能撤销排序。
        // 把当前内容再排一遍做不到这件事。
        let mut t = JsonTool::default();
        t.input.set_text(r#"{"b":1,"a":2,"c":3}"#);
        t.format();
        let original = t.input.text();
        assert!(
            original.find("\"b\"").unwrap() < original.find("\"a\"").unwrap(),
            "前提：原始键序是 b 在 a 前"
        );

        t.toggle_sort();
        assert!(t.sort);
        let sorted = t.input.text();
        assert!(
            sorted.find("\"a\"").unwrap() < sorted.find("\"b\"").unwrap(),
            "排序没生效：{sorted}"
        );

        t.toggle_sort();
        assert!(!t.sort);
        assert_eq!(t.input.text(), original, "关掉排序没有退回原始键序");
    }

    #[test]
    fn sorting_invalid_json_reverts_the_toggle() {
        // 否则界面显示「已排序」而内容一个字没变。
        let mut t = JsonTool::default();
        t.input.set_text("not json");
        t.toggle_sort();
        assert!(!t.sort, "排不动时开关必须退回去");
        assert!(!t.ok);
    }

    #[test]
    fn search_finds_every_occurrence_by_char_index() {
        let mut t = JsonTool::default();
        t.input.set_text("aXbXcX");
        t.set_find("X");
        t.search();
        assert_eq!(t.hits, vec![1, 3, 5]);
    }

    #[test]
    fn search_positions_are_char_based_not_byte_based() {
        // 中文一个字 3 字节。用字节位置会在中文文本上错位，
        // 表现是「点下一个命中，光标跳到别处」。
        let mut t = JsonTool::default();
        t.input.set_text("中文X中文X");
        t.set_find("X");
        t.search();
        assert_eq!(t.hits, vec![2, 5], "命中位置必须是 char 索引");
    }

    #[test]
    fn next_hit_wraps_around() {
        let mut t = JsonTool::default();
        t.input.set_text("XX");
        t.set_find("X");
        t.search();
        assert_eq!(t.hit_idx, 0);
        t.next_hit();
        assert_eq!(t.hit_idx, 1);
        t.next_hit();
        assert_eq!(t.hit_idx, 0, "到末尾应当循环回第一个");
        t.prev_hit();
        assert_eq!(t.hit_idx, 1, "往前也应当循环");
    }

    #[test]
    fn missing_needle_says_so_without_clearing_the_document() {
        let mut t = JsonTool::default();
        let before = t.input.text();
        t.set_find("绝不存在的串");
        t.search();
        assert!(t.hits.is_empty());
        assert!(t.status.contains("未找到"));
        assert_eq!(t.input.text(), before);
    }

    #[test]
    fn editing_invalidates_stale_hits() {
        // 留着旧命中位置会让「下一个」跳到错误的地方。
        let mut t = JsonTool::default();
        t.input.set_text("XaX");
        t.set_find("X");
        t.search();
        assert!(!t.hits.is_empty());
        t.input.insert_char('Z');
        t.on_edited();
        assert!(t.hits.is_empty(), "编辑后旧命中必须作废");
    }

    #[test]
    fn undo_redo_goes_through_the_buffer() {
        let mut t = JsonTool::default();
        let before = t.input.text();
        t.input.insert_str("XYZ");
        t.on_edited();
        assert_ne!(t.input.text(), before);
        t.undo();
        assert_eq!(t.input.text(), before);
        t.redo();
        assert_ne!(t.input.text(), before);
    }

    #[test]
    fn formatting_keeps_the_scroll_position() {
        let big: String = format!(
            "[{}]",
            (0..3000)
                .map(|i| format!("{{\"i\":{i}}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut t = JsonTool::default();
        t.input.set_text(&big);
        t.format();
        t.input.set_viewport(25, 100);
        t.input.scroll_to_line(1000);
        t.format();
        assert_eq!(t.input.scroll_line(), 1000, "格式化后视野跳走了");
    }

    #[test]
    fn a_huge_document_stays_virtualized() {
        // 崩溃守门：Slint 原生 TextEdit 在约 2190 行以上 panic。
        let big: String = format!(
            "[{}]",
            (0..20_000)
                .map(|i| format!("{{\"i\":{i}}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut t = JsonTool::default();
        t.input.set_text(&big);
        t.format();
        t.input.set_viewport(30, 120);
        assert!(
            t.input.total_lines() > 2190,
            "实际 {}",
            t.input.total_lines()
        );
        assert_eq!(t.input.visible_lines().len(), 30);
        assert!(t.ok, "{}", t.status);
    }

    #[test]
    fn draft_roundtrip_preserves_everything() {
        let mut t = JsonTool::default();
        t.input.set_text(r#"{"z":1}"#);
        t.set_indent_index(2);
        t.toggle_wrap();
        let saved = t.save_draft().expect("必须持久化草稿");

        let mut r = JsonTool::default();
        r.load_draft(&saved);
        assert_eq!(r.input.text(), r#"{"z":1}"#);
        assert_eq!(r.indent, Indent::Tab);
        assert!(!r.wrap);
        assert!(r.ok, "恢复草稿后应当已经校验过");
    }

    #[test]
    fn legacy_draft_without_wrap_defaults_to_on() {
        // 老草稿没有 wrap 字段。默认关的话老用户升级后长行突然看不见了。
        let mut t = JsonTool::default();
        t.load_draft(r#"{"input":"{}","indent":"Two","sort":false}"#);
        assert!(t.wrap);
    }
}
