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
    /// 关键字大写（关 = 小写）。
    pub uppercase: bool,
    pub status: String,
    /// 上一次格式化/压缩吐出的文本。缓冲区还与它逐字相同 = 用户没改过，
    /// 这时候切大小写可以**当场重排**（切了没反应正是这个开关被嫌弃的原因）；
    /// 一旦有人动过一个字符就不再自动重排 —— 那会改掉他手写的 SQL。
    last_out: String,
}

impl Default for SqlTool {
    fn default() -> Self {
        Self {
            input: TextBuffer::new(SAMPLE),
            uppercase: true,
            status: "就绪".to_owned(),
            last_out: String::new(),
        }
    }
}

impl SqlTool {
    fn case(&self) -> sql::Case {
        if self.uppercase {
            sql::Case::Upper
        } else {
            sql::Case::Lower
        }
    }

    pub fn format(&mut self) {
        let out = sql::format(&self.input.text(), self.case());
        self.input.replace_keeping_view(&out);
        self.last_out = out;
        self.status = "已格式化".to_owned();
    }

    pub fn minify(&mut self) {
        let out = sql::minify(&self.input.text());
        self.input.replace_keeping_view(&out);
        // 压缩结果不是排版结果：此后切大小写不该把它重新展开成多行。
        self.last_out.clear();
        self.status = "已压缩为单行".to_owned();
    }

    /// 选关键字大小写（0 大写 / 1 小写）。
    ///
    /// 内容还是上一次排版的原样就当场重排，否则只记下设置、等下次格式化 ——
    /// 手写的 SQL 不该在用户没按格式化的时候被改写。
    pub fn set_case(&mut self, index: i32) {
        let want = index != 1;
        if want == self.uppercase {
            return;
        }
        self.uppercase = want;
        let untouched = !self.last_out.is_empty() && self.input.text() == self.last_out;
        if untouched {
            self.format();
            self.status = if self.uppercase {
                "关键字已改为大写".to_owned()
            } else {
                "关键字已改为小写".to_owned()
            };
        } else {
            self.status = if self.uppercase {
                "关键字大写：开（下次格式化生效）".to_owned()
            } else {
                "关键字大写：关（下次格式化生效）".to_owned()
            };
        }
    }

    pub fn clear(&mut self) {
        self.input.set_text("");
        self.last_out.clear();
        self.status = "已清空".to_owned();
    }
}

impl Tool for SqlTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "sql",
            name: "SQL 格式化",
            desc: "美化 / 压缩 SQL，关键字换行缩进，关键字大写 / 小写随时切。",
            icon: icons::DATABASE,
            group: "SQL",
            keywords: &["sql", "format", "格式化", "美化"],
        }
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
    fn lower_case_actually_lowercases_on_the_next_format() {
        let mut t = SqlTool::default();
        t.set_case(1);
        assert!(!t.uppercase);
        t.format();
        let out = t.input.text();
        assert!(out.contains("select"), "{out}");
        assert!(!out.contains("SELECT"), "关掉大写后关键字还是大写：{out}");
    }

    #[test]
    fn switching_case_reformats_untouched_output_on_the_spot() {
        // 「切了没反应」是这个开关被嫌弃的根因：内容还是上次排版的原样时，
        // 切换必须当场看到效果。
        let mut t = SqlTool::default();
        t.format();
        assert!(t.input.text().contains("SELECT"));

        t.set_case(1);
        let out = t.input.text();
        assert!(out.contains("select"), "切到小写没有当场生效：{out}");
        assert!(!out.contains("SELECT"), "{out}");
        assert_eq!(t.status, "关键字已改为小写");

        t.set_case(0);
        assert!(t.input.text().contains("SELECT"), "切回大写没生效");
        assert_eq!(t.status, "关键字已改为大写");
    }

    #[test]
    fn switching_case_never_rewrites_hand_written_sql() {
        // 用户自己敲的内容不该在没按格式化的时候被改掉。
        let mut t = SqlTool::default();
        t.input.set_text("select  Id   from  T -- 我自己排的");
        let before = t.input.text();
        t.set_case(1);
        assert_eq!(t.input.text(), before, "手写内容被自动重排了");
        assert!(t.status.contains("下次格式化生效"), "{}", t.status);
    }

    #[test]
    fn switching_case_after_minify_keeps_it_on_one_line() {
        // 压缩过的单行不是排版结果，切大小写不该把它重新展开。
        let mut t = SqlTool::default();
        t.format();
        t.minify();
        let before = t.input.text();
        t.set_case(1);
        assert_eq!(t.input.text(), before);
    }

    #[test]
    fn re_selecting_the_same_case_is_a_noop() {
        let mut t = SqlTool::default();
        t.format();
        let before = t.input.text();
        t.set_case(0);
        assert_eq!(t.input.text(), before);
        assert_eq!(t.status, "已格式化", "同一档重复选中不该改状态");
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
        t.set_case(1);
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
