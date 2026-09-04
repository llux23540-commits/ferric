//! 工具视图集合与注册表。
//!
//! 全部 11 个内置工具都已迁到 Slint：视图在 `ui/app.slint`，状态与业务在
//! 各自的 `views/*.rs`。
//!
//! 新增一个工具 = 写 `views/<id>.rs`（形状参照 `views/uuid.rs`：状态 + 业务
//! + 索引映射 + `migrated() -> true`）+ 在 `ui/app.slint` 里加视图组件与分支
//! + 在这里注册一行 + 在 `state.rs` 的 `Shell::with_buffer` 里加编辑区映射。
//! 侧栏顺序 = 此处顺序，与 egui 版逐项对齐，不要重排。
//!
//! `every_builtin_tool_is_migrated` 守着「新工具别忘了实现视图」。

mod crypto;
mod diff;
mod gm;
mod json;
mod market;
mod regex;
mod rsa;
mod sql;
mod timestamp;
mod uuid;
mod yaml;

pub use crypto::CryptoTool;
pub use diff::DiffTool;
pub use gm::GmTool;
pub use json::JsonTool;
pub use market::MarketTool;
pub use regex::RegexTool;
pub use rsa::RsaTool;
pub use sql::SqlTool;
pub use timestamp::TimestampTool;
pub use uuid::UuidTool;
pub use yaml::YamlTool;

use crate::icons;
use crate::tool::Tool;
/// 全部工具的注册表。侧栏顺序即此顺序（与 egui 版逐项对齐）。
pub fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(JsonTool::default()),
        Box::new(DiffTool::default()),
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
        Box::new(MarketTool::default()),
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
    fn every_builtin_tool_is_migrated() {
        // 迁移收尾的守门人：内置工具**全部**已有 Slint 视图。
        // 将来新增工具若忘了实现视图，这条会红。
        let pending: Vec<&str> = registry()
            .iter()
            .filter(|t| !t.migrated())
            .map(|t| t.meta().id)
            .collect();
        assert!(
            pending.is_empty(),
            "这些内置工具还没有 Slint 视图：{pending:?}"
        );
    }
}