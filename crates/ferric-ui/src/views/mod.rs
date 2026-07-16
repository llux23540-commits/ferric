//! 工具视图集合与注册表。

mod crypto;
mod gm;
mod json;
mod regex;
mod rsa;
mod sql;
mod timestamp;
mod uuid;
mod yaml;

use crate::tool::Tool;

/// 全部工具的注册表。侧栏顺序即此顺序（按 group 保序分组）。
pub fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(json::JsonTool::default()),
        Box::new(timestamp::TimestampTool::default()),
        Box::new(yaml::YamlTool::default()),
        Box::new(sql::SqlTool::default()),
        Box::new(uuid::UuidTool::default()),
        Box::new(rsa::RsaTool::default()),
        Box::new(crypto::CryptoTool::default()),
        Box::new(gm::GmTool::default()),
        Box::new(regex::RegexTool::default()),
    ]
}
