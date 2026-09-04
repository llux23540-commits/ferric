//! 尚未迁到 Slint 的工具：**状态与草稿契约照旧，只差视图层**。
//!
//! 为什么不是「先删掉，迁到时再加回来」：
//!
//! 1. 侧栏必须保持原有的 11 项 —— 用户升级后打开看到工具少了 9 个，那是回归，
//!    不是「迁移中」；
//! 2. `Persist.drafts` 是按工具 id 存的 `HashMap<String, String>`。如果这一版
//!    不注册这些 id，外壳落盘时就会把它们的草稿**清掉** —— 用户在 egui 版
//!    存的 JSON 输入、diff 文本会静默消失。这里原样收着字符串、原样写回去，
//!    迁完视图接上即恢复；
//! 3. 迁移顺序可以随时调整，不牵动状态层。
//!
//! 每迁完一个工具：把它从这里摘掉，改成独立的 `views/<id>.rs`（参照
//! `views/uuid.rs` 的形状：状态 + 业务 + 索引映射 + `migrated() -> true`）。

use crate::tool::{Tool, ToolMeta};

/// 一个「视图待迁移」的工具。业务逻辑还在 `ferric-core` 里，
/// 草稿只做透传保存（不解析 —— 解析是视图层的事，这里没有视图）。
pub struct PendingTool {
    meta: ToolMeta,
    /// 原样收着上一版存下来的草稿，落盘时原样写回。
    raw_draft: Option<String>,
}

impl PendingTool {
    pub fn new(meta: ToolMeta) -> Self {
        Self {
            meta,
            raw_draft: None,
        }
    }
}

impl Tool for PendingTool {
    fn meta(&self) -> ToolMeta {
        self.meta
    }

    fn migrated(&self) -> bool {
        false
    }

    /// 透传：把读进来的那份草稿原样交回去落盘。
    ///
    /// 返回 `None` 会让外壳把这个 id 从 `drafts` 里删掉 —— 那正是要避免的，
    /// 所以只有「本来就没有草稿」时才返回 `None`。
    fn save_draft(&self) -> Option<String> {
        self.raw_draft.clone()
    }

    fn load_draft(&mut self, data: &str) {
        self.raw_draft = Some(data.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons;

    fn meta() -> ToolMeta {
        ToolMeta {
            id: "json",
            name: "JSON 工具",
            desc: "占位",
            icon: icons::BRACES,
            group: "格式",
            keywords: &["json"],
        }
    }

    #[test]
    fn draft_passes_through_untouched() {
        // 这是本文件存在的理由：egui 版存的草稿在「视图还没迁」的这一版里
        // 必须原样活下来，否则用户的输入静默丢失。
        let payload = r#"{"input":"{\"a\":1}","indent":4,"wrap":true}"#;
        let mut t = PendingTool::new(meta());
        t.load_draft(payload);
        assert_eq!(t.save_draft().as_deref(), Some(payload));
    }

    #[test]
    fn no_draft_stays_none_so_shell_writes_nothing() {
        let t = PendingTool::new(meta());
        assert_eq!(t.save_draft(), None);
    }
}