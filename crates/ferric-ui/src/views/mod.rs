//! 工具视图集合与注册表。
//!
//! 迁移状态：`uuid` 已迁到 Slint；其余 10 个走 [`pending::PendingTool`]
//! （侧栏照旧显示、草稿照旧保存，只是主区显示「正在迁移」）。
//!
//! 迁完一个工具的做法：写 `views/<id>.rs`（形状参照 `views/uuid.rs`），
//! 在 `ui/app.slint` 里加对应的视图组件与分支，然后把这里的 `pending(...)`
//! 换成真实构造。侧栏顺序 = 此处顺序，与 egui 版一致，不要重排。

mod crypto;
mod gm;
mod json;
mod pending;
mod regex;
mod rsa;
mod sql;
mod timestamp;
mod uuid;
mod yaml;

pub use crypto::CryptoTool;
pub use gm::GmTool;
pub use json::JsonTool;
pub use pending::PendingTool;
pub use regex::RegexTool;
pub use rsa::RsaTool;
pub use sql::SqlTool;
pub use timestamp::TimestampTool;
pub use uuid::UuidTool;
pub use yaml::YamlTool;

use crate::icons;
use crate::tool::{Tool, ToolMeta};
fn pending(
    id: &'static str,
    name: &'static str,
    desc: &'static str,
    icon: char,
    group: &'static str,
    keywords: &'static [&'static str],
) -> Box<dyn Tool> {
    Box::new(PendingTool::new(ToolMeta {
        id,
        name,
        desc,
        icon,
        group,
        keywords,
    }))
}

/// 全部工具的注册表。侧栏顺序即此顺序（与 egui 版逐项对齐）。
pub fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(JsonTool::default()),
        pending(
            "diff",
            "文本 / 文件对比",
            "逐行 diff，差异高亮在左右面板内，左右同步滚动，载入 / 拖入文件",
            icons::GIT_COMPARE,
            "格式",
            &["diff", "compare", "对比", "比较", "差异"],
        ),
        Box::new(TimestampTool::default()),
        // ——— 已迁移 ———
        Box::new(YamlTool::default()),
        Box::new(SqlTool::default()),
        // ——— 已迁移 ———
        Box::new(UuidTool::default()),
        Box::new(RsaTool::default()),
        Box::new(CryptoTool::default()),
        Box::new(GmTool::default()),
        Box::new(RegexTool::default()),
        pending(
            "market",
            "插件市场",
            "浏览并安装 WASM 插件，全部更新，签名校验",
            icons::BOX,
            "扩展",
            &["plugin", "market", "插件", "市场", "扩展"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_keeps_all_eleven_tools_visible_during_migration() {
        // 迁移期间侧栏不能少工具 —— 少了对用户就是功能回归。
        assert_eq!(registry().len(), 11);
    }

    #[test]
    fn tool_ids_are_unique_and_match_legacy_set() {
        // id 是 Persist.drafts 的键。改一个就等于把老用户那条草稿孤立掉。
        let ids: Vec<&str> = registry().iter().map(|t| t.meta().id).collect();
        let uniq: HashSet<&&str> = ids.iter().collect();
        assert_eq!(uniq.len(), ids.len(), "工具 id 必须唯一");

        let legacy: HashSet<&str> = [
            "json",
            "diff",
            "timestamp",
            "yaml",
            "sql",
            "uuid",
            "rsa",
            "crypto",
            "gm",
            "regex",
            "market",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            ids.into_iter().collect::<HashSet<&str>>(),
            legacy,
            "id 集合必须与 egui 版完全一致，否则老用户草稿会丢"
        );
    }

    #[test]
    fn migrated_set_is_explicit() {
        let migrated: Vec<&str> = registry()
            .iter()
            .filter(|t| t.migrated())
            .map(|t| t.meta().id)
            .collect();
        assert_eq!(
            migrated,
            vec!["json", "timestamp", "yaml", "sql", "uuid", "rsa", "crypto", "gm", "regex"],
            "迁完一个就在这里加一项"
        );
    }
}