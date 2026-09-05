//! JSON → YAML 转换 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `YamlView`（两个 [`CodeEditor`]：左输入右输出）。
//! 转换逻辑全在 `ferric_core::yaml`（无 GUI 依赖，带单测），没动过。
//!
//! 输入框用自管的 [`crate::editor::TextBuffer`] 而不是 Slint 的 `TextEdit`：
//! 后者超过约 2190 行会 panic（见 `editor` 模块文档）。用户往 JSON 框里粘一份
//! 大接口响应是完全正常的操作，不能崩。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::yaml;
use serde::{Deserialize, Serialize};

const SAMPLE: &str = "{\"name\":\"ferric\",\"tags\":[\"json\",\"yaml\"],\"ok\":true}";

#[derive(Serialize, Deserialize)]
struct YamlDraft {
    input: String,
}

pub struct YamlTool {
    /// 输入缓冲区（rope + 光标 + 选区 + 滚动）。
    pub input: TextBuffer,
    /// 输出缓冲区。只读，但同样虚拟化 —— YAML 输出可能比输入还长。
    pub output: TextBuffer,
    pub ok: bool,
    pub status: String,
}

impl Default for YamlTool {
    fn default() -> Self {
        let mut t = Self {
            input: TextBuffer::new(SAMPLE),
            output: TextBuffer::new(""),
            ok: true,
            status: String::new(),
        };
        t.convert();
        t
    }
}

impl YamlTool {
    /// 实时转换。失败时**不清空输出** —— 用户正在打字，中间态必然不是合法
    /// JSON，把上一次的好结果冲掉会让右侧一直在闪。
    pub fn convert(&mut self) {
        let src = self.input.text();
        if src.trim().is_empty() {
            self.output.set_text("");
            self.ok = true;
            self.status = "就绪 —— 输入 JSON 后实时转换".to_owned();
            return;
        }
        match yaml::json_to_yaml(&src) {
            Ok(y) => {
                self.output.replace_keeping_view(&y);
                self.ok = true;
                self.status = "JSON 有效 · 已实时转换".to_owned();
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("解析失败：{e}");
            }
        }
    }

    pub fn load_sample(&mut self) {
        self.input.set_text(SAMPLE);
        self.convert();
    }

    pub fn clear(&mut self) {
        self.input.set_text("");
        self.convert();
    }
}

impl Tool for YamlTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "yaml",
            name: "JSON → YAML",
            desc: "简单地将 JSON 转换为 YAML —— 左侧输入 JSON，右侧实时输出 YAML。",
            icon: icons::LIST_CHECKS,
            group: "转换",
            keywords: &["yaml", "json", "转换", "convert"],
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&YamlDraft {
            input: self.input.text(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<YamlDraft>(data) {
            self.input.set_text(&d.input);
            self.convert();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json_converts_to_yaml() {
        let t = YamlTool::default();
        assert!(t.ok, "示例 JSON 必须转换成功：{}", t.status);
        let out = t.output.text();
        assert!(out.contains("name: ferric"), "输出不像 YAML：{out}");
    }

    #[test]
    fn invalid_json_keeps_the_last_good_output() {
        // 用户打字过程中必然经过非法中间态。那时把右侧清空 = 一直在闪。
        let mut t = YamlTool::default();
        let good = t.output.text();
        assert!(!good.is_empty());

        t.input.set_text("{\"broken\": ");
        t.convert();
        assert!(!t.ok, "非法 JSON 必须报错");
        assert_eq!(t.output.text(), good, "失败时不能把上一次的好输出冲掉");
        assert!(t.status.contains("解析失败"));
    }

    #[test]
    fn empty_input_clears_output_and_says_ready() {
        // 空输入不是错误 —— 那是初始状态，不该显示红字。
        let mut t = YamlTool::default();
        t.input.set_text("   \n  ");
        t.convert();
        assert!(t.ok);
        assert!(t.output.is_empty());
        assert!(t.status.contains("就绪"));
    }

    #[test]
    fn draft_roundtrip_preserves_input() {
        let mut t = YamlTool::default();
        t.input.set_text("{\"a\": [1, 2, 3]}");
        t.convert();
        let saved = t.save_draft().expect("必须持久化草稿");

        let mut restored = YamlTool::default();
        restored.load_draft(&saved);
        assert_eq!(restored.input.text(), "{\"a\": [1, 2, 3]}");
        assert!(restored.ok, "恢复草稿后应当已经转换过");
        assert!(restored.output.text().contains("- 1"));
    }

    #[test]
    fn a_huge_document_stays_virtualized() {
        // 这条守的是崩溃风险：Slint 原生 TextEdit 在约 2190 行以上 panic。
        // 输入输出都必须只把视口那几行交给渲染层。
        let big: String = format!(
            "[{}]",
            (0..20_000)
                .map(|i| format!("{{\"i\":{i}}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut t = YamlTool::default();
        t.input.set_text(&big);
        t.input.set_viewport(30, 100);
        t.convert();
        assert!(t.ok, "两万条数组应当转换成功：{}", t.status);

        t.output.set_viewport(30, 100);
        // 输出的 YAML 是每键一行 —— 两万条对象远超原生 TextEdit 的 ~2190 行上限。
        assert!(
            t.output.total_lines() > 2190,
            "输出只有 {} 行，测不到虚拟化",
            t.output.total_lines()
        );
        // 关键断言：不管文档多大，交给渲染层的永远只是视口那几十行。
        assert_eq!(t.output.visible_lines().len(), 30);
        // 输入是单行 JSON（接口响应常态），行数少于视口时按实际行数给 ——
        // 这也正确：渲染层拿到的行数永远 ≤ 视口高度。
        assert!(t.input.visible_lines().len() <= 30);
    }
}
