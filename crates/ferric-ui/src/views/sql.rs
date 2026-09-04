//! SQL 格式化 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `SqlView`（单个虚拟化编辑区 + 工具条）。
//! 格式化逻辑全在 `ferric_core::sql`（无 GUI 依赖，带单测），没动过。
//!
//! 这是**原地编辑**型工具：格式化/压缩的结果写回同一个缓冲区，所以用
//! [`TextBuffer::replace_keeping_view`] 而不是 `set_text` —— 用户在第 500 行
//! 按格式化，视野不该跳回顶部。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::sql;
use serde::{Deserialize, Serialize};

const SAMPLE: &str = "select id,name,email from users where age>18 order by name";

#[derive(Serialize, Deserialize)]
struct SqlDraft {
    input: String,
    uppercase: bool,
}

pub struct SqlTool {
    pub input: TextBuffer,
    pub uppercase: bool,
    pub status: String,
}

impl Default for SqlTool {
    fn default() -> Self {
        Self {
            input: TextBuffer::new(SAMPLE),
            uppercase: true,
            status: "就绪".to_owned(),
        }
    }
}

impl SqlTool {
    pub fn format(&mut self) {
        let out = sql::format(&self.input.text(), self.uppercase);
        self.input.replace_keeping_view(&out);
        self.status = "已格式化".to_owned();
    }

    pub fn minify(&mut self) {
        let out = sql::minify(&self.input.text());
        self.input.replace_keeping_view(&out);
        self.status = "已压缩为单行".to_owned();
    }

    /// 切换「关键字大写」。**不自动重排** —— 那会在用户还没看清的时候
    /// 改掉他手写的 SQL；下次按格式化才生效，与 egui 版行为一致。
    pub fn toggle_uppercase(&mut self) {
        self.uppercase = !self.uppercase;
        self.status = if self.uppercase {
            "关键字大写：开（下次格式化生效）".to_owned()
        } else {
            "关键字大写：关（下次格式化生效）".to_owned()
        };
    }

    pub fn clear(&mut self) {
        self.input.set_text("");
        self.status = "已清空".to_owned();
    }
}

impl Tool for SqlTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "sql",
            name: "SQL 格式化",
            desc: "美化 / 压缩 SQL，关键字换行缩进，可选关键字大写。",
            icon: icons::DATABASE,
            group: "SQL",
            keywords: &["sql", "format", "格式化", "美化"],
        }
    }

    fn migrated(&self) -> bool {
        true
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&SqlDraft {
            input: self.input.text(),
            uppercase: self.uppercase,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<SqlDraft>(data) {
            self.input.set_text(&d.input);
            self.uppercase = d.uppercase;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_expands_the_sample_into_multiple_lines() {
        let mut t = SqlTool::default();
        assert_eq!(t.input.total_lines(), 1, "示例本是单行");
        t.format();
        assert!(t.input.total_lines() > 1, "格式化后应当换行缩进");
        assert!(t.input.text().contains("SELECT"), "开了大写却没大写");
    }

    #[test]
    fn minify_collapses_back_to_one_line() {
        let mut t = SqlTool::default();
        t.format();
        assert!(t.input.total_lines() > 1);
        t.minify();
        assert_eq!(
            t.input.text().trim().lines().count(),
            1,
            "压缩后应当只有一行"
        );
    }

    #[test]
    fn uppercase_off_keeps_keywords_as_written() {
        let mut t = SqlTool::default();
        t.toggle_uppercase();
        assert!(!t.uppercase);
        t.format();
        assert!(
            !t.input.text().contains("SELECT"),
            "关掉大写后不该把关键字改成大写：{}",
            t.input.text()
        );
    }

    #[test]
    fn toggling_uppercase_does_not_reformat_immediately() {
        // 立刻重排会在用户还没看清的时候改掉他手写的 SQL。
        let mut t = SqlTool::default();
        let before = t.input.text();
        t.toggle_uppercase();
        assert_eq!(t.input.text(), before);
        assert!(t.status.contains("下次格式化生效"));
    }

    #[test]
    fn formatting_keeps_the_scroll_position() {
        // 用户在第 300 行按格式化，视野不该跳回顶部。
        let long: String = (0..600)
            .map(|i| format!("select c{i} from t{i};\n"))
            .collect();
        let mut t = SqlTool::default();
        t.input.set_text(&long);
        t.input.set_viewport(20, 100);
        t.input.scroll_to_line(300);
        t.format();
        assert_eq!(t.input.scroll_line(), 300, "格式化后视野跳走了");
    }

    #[test]
    fn draft_roundtrip_preserves_input_and_flag() {
        let mut t = SqlTool::default();
        t.input.set_text("select 1");
        t.toggle_uppercase();
        let saved = t.save_draft().expect("必须持久化草稿");

        let mut restored = SqlTool::default();
        restored.load_draft(&saved);
        assert_eq!(restored.input.text(), "select 1");
        assert!(!restored.uppercase, "大写开关没跟着草稿恢复");
    }

    #[test]
    fn a_huge_script_stays_virtualized() {
        // 崩溃守门：Slint 原生 TextEdit 在约 2190 行以上 panic。
        let long: String = (0..8000)
            .map(|i| format!("insert into t values ({i});\n"))
            .collect();
        let mut t = SqlTool::default();
        t.input.set_text(&long);
        t.input.set_viewport(28, 100);
        assert!(t.input.total_lines() > 2190);
        assert_eq!(t.input.visible_lines().len(), 28);
    }
}