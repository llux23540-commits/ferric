//! 插件清单（manifest）schema —— 声明式表单 + 元数据。
//!
//! 插件是编译到 wasm32 的模块，导出四个符号：
//! `ferric_alloc(len)->ptr`、`ferric_dealloc(ptr,len)`、
//! `ferric_manifest()->packed(ptr,len)`、`ferric_process(ptr,len)->packed(ptr,len)`；
//! 一切数据以 UTF-8 JSON 传递。宿主按本模块的 [`Manifest`] 渲染表单、
//! 组装 [`ProcessIn`]、解析 [`ProcessOut`]。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 宿主当前支持的插件接口版本。破坏性变更时 +1。
pub const API_VERSION: u32 = 1;

/// 插件元数据 + 表单声明。由插件的 `ferric_manifest()` 返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub api_version: u32,
    /// 唯一 id（ASCII 字母数字 / `-` / `_`），用于草稿、收藏等持久化键。
    pub id: String,
    pub name: String,
    #[serde(default = "default_group")]
    pub group: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// 输入框标题（缺省「输入」）。
    #[serde(default)]
    pub input_label: Option<String>,
    /// 输出框标题（缺省「输出」）。
    #[serde(default)]
    pub output_label: Option<String>,
    #[serde(default)]
    pub options: Vec<OptionSpec>,
}

fn default_group() -> String {
    "插件".to_owned()
}

/// 表单选项声明。宿主用现成组件渲染，值以字符串传给插件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OptionSpec {
    /// 分段单选：值为选中项文本。
    Seg {
        key: String,
        label: String,
        values: Vec<String>,
        #[serde(default)]
        default: usize,
    },
    /// 开关：值为 "true" / "false"。
    Toggle {
        key: String,
        label: String,
        #[serde(default)]
        default: bool,
    },
    /// 单行文本。
    Text {
        key: String,
        label: String,
        #[serde(default)]
        default: String,
        #[serde(default)]
        hint: String,
    },
}

impl OptionSpec {
    pub fn key(&self) -> &str {
        match self {
            OptionSpec::Seg { key, .. }
            | OptionSpec::Toggle { key, .. }
            | OptionSpec::Text { key, .. } => key,
        }
    }
}

impl Manifest {
    /// 校验清单合法性；错误信息面向插件作者。
    pub fn validate(&self) -> Result<(), String> {
        if self.api_version != API_VERSION {
            return Err(format!(
                "接口版本不匹配：插件为 v{}，宿主支持 v{API_VERSION}",
                self.api_version
            ));
        }
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("id 须为非空 ASCII 字母数字 / - / _".into());
        }
        if self.name.trim().is_empty() {
            return Err("name 不能为空".into());
        }
        let mut seen = std::collections::HashSet::new();
        for o in &self.options {
            if o.key().is_empty() {
                return Err("选项 key 不能为空".into());
            }
            if !seen.insert(o.key().to_owned()) {
                return Err(format!("选项 key 重复：{}", o.key()));
            }
            if let OptionSpec::Seg {
                key,
                values,
                default,
                ..
            } = o
            {
                if values.is_empty() || *default >= values.len() {
                    return Err(format!("seg 选项 {key} 的 values/default 非法"));
                }
            }
        }
        Ok(())
    }
}

/// process 的入参（宿主 → 插件）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessIn {
    pub input: String,
    /// 选项值（key → 字符串值）。
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

/// process 的出参（插件 → 宿主）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOut {
    pub ok: bool,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Manifest {
        serde_json::from_str(
            r#"{
                "api_version": 1, "id": "demo", "name": "示例",
                "options": [
                    {"kind":"seg","key":"mode","label":"方向","values":["编码","解码"]},
                    {"kind":"toggle","key":"upper","label":"大写"},
                    {"kind":"text","key":"sep","label":"分隔符","default":","}
                ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn manifest_parses_and_validates() {
        let m = base();
        assert!(m.validate().is_ok());
        assert_eq!(m.group, "插件"); // 默认分组
        assert_eq!(m.options.len(), 3);
    }

    #[test]
    fn manifest_rejects_bad() {
        let mut m = base();
        m.api_version = 999;
        assert!(m.validate().is_err());

        let mut m = base();
        m.id = "非法 id".into();
        assert!(m.validate().is_err());

        let mut m = base();
        if let OptionSpec::Seg { default, .. } = &mut m.options[0] {
            *default = 9;
        }
        assert!(m.validate().is_err());
    }

    #[test]
    fn process_io_roundtrip() {
        let json = r#"{"input":"abc","options":{"mode":"编码"}}"#;
        let p: ProcessIn = serde_json::from_str(json).unwrap();
        assert_eq!(p.input, "abc");
        assert_eq!(p.options["mode"], "编码");
        let out: ProcessOut = serde_json::from_str(r#"{"ok":true,"output":"x"}"#).unwrap();
        assert!(out.ok && out.error.is_empty());
    }
}
