//! 工具视图集合与注册表。
//!
//! 迁移状态：`uuid` 已迁到 Slint；其余 10 个走 [`pending::PendingTool`]
//! （侧栏照旧显示、草稿照旧保存，只是主区显示「正在迁移」）。
//!
//! 迁完一个工具的做法：写 `views/<id>.rs`（形状参照 `views/uuid.rs`），
//! 在 `ui/app.slint` 里加对应的视图组件与分支，然后把这里的 `pending(...)`
//! 换成真实构造。侧栏顺序 = 此处顺序，与 egui 版一致，不要重排。

mod pending;
mod uuid;
mod yaml;

pub use pending::PendingTool;
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
        pending(
            "json",
            "JSON 工具",
            "格式化 / 压缩 / 校验 / 转义 / 去转义 / 键名排序，搜索、折叠树视图、撤销重做",
            icons::BRACES,
            "格式",
            &["json", "format", "beautify", "minify", "美化", "格式化", "压缩"],
        ),
        pending(
            "diff",
            "文本 / 文件对比",
            "逐行 diff，差异高亮在左右面板内，左右同步滚动，载入 / 拖入文件",
            icons::GIT_COMPARE,
            "格式",
            &["diff", "compare", "对比", "比较", "差异"],
        ),
        pending(
            "timestamp",
            "时间戳",
            "Unix ↔ 日期时间，秒/毫秒，全量时区可搜索，自动识别多种日期格式",
            icons::CLOCK,
            "转换",
            &["timestamp", "unix", "时间戳", "时间", "date", "时区"],
        ),
        // ——— 已迁移 ———
        Box::new(YamlTool::default()),
        pending(
            "sql",
            "SQL 格式化",
            "格式化 / 压缩为单行，关键字大写开关",
            icons::DATABASE,
            "格式",
            &["sql", "format", "格式化", "美化"],
        ),
        // ——— 已迁移 ———
        Box::new(UuidTool::default()),
        pending(
            "rsa",
            "RSA 密钥对",
            "256–4096 位，后台线程生成，PEM 输出",
            icons::KEY,
            "加密",
            &["rsa", "key", "密钥", "pem", "公钥", "私钥"],
        ),
        pending(
            "crypto",
            "加密 / 解密文本",
            "AES / TripleDES / Rabbit / RC4，OpenSSL 盐格式，与 crypto-js 兼容",
            icons::LOCK,
            "加密",
            &["crypto", "aes", "加密", "解密", "encrypt"],
        ),
        pending(
            "gm",
            "国密 SM",
            "SM4 对称、SM2 公钥加解密、SM3 摘要，一键生成 SM2 密钥对",
            icons::SHIELD_CHECK,
            "加密",
            &["gm", "国密", "sm2", "sm3", "sm4", "国密sm"],
        ),
        pending(
            "regex",
            "正则表达式",
            "g/i/m/s/x 标志，分组捕获展示，常用语法备忘单",
            icons::TERMINAL,
            "文本",
            &["regex", "正则", "regexp", "match"],
        ),
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
        assert_eq!(migrated, vec!["yaml", "uuid"], "迁完一个就在这里加一项");
    }
}