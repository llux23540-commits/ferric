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
    ///
    /// **同时也是插件市场里的 slug**：宿主安装插件时固定写成 `<id>.wasm`，
    /// 更新检查也用它向服务端提问。两者必须一致，否则装完认不出是同一个插件。
    pub id: String,
    /// 插件自身的语义版本号（如 `1.2.0`）。
    ///
    /// 加了 `serde(default)`，所以**老插件不带这个字段也能正常加载**，只是版本为空串 ——
    /// 这正好对应服务端 `check-updates` 的约定「读不出来时传空串，一律按有更新处理」。
    /// 版本号写在 wasm 内部而不是文件名/旁挂文件，是为了让它和代码物理绑定，
    /// 不可能出现「文件说是 1.2.0、里面其实是 1.0.0」这种不一致。
    #[serde(default)]
    pub version: String,
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
        // 版本号可以不填（老插件），但填了就得像个版本号 —— 服务端上架时要求合法
        // semver，这里只做轻量形状检查，避免为此引入 semver 依赖。
        if !self.version.is_empty() {
            let v = self.version.trim();
            let shape_ok = v.split('.').count() >= 3
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
                && v.starts_with(|c: char| c.is_ascii_digit());
            if !shape_ok {
                return Err(format!(
                    "version 须为 semver 形式（如 1.2.0），当前为「{v}」"
                ));
            }
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

    /// 老插件（manifest 里没有 version）必须照常加载 —— 这是加字段时最重要的兼容性保证。
    #[test]
    fn legacy_manifest_without_version_still_loads() {
        let m: Manifest =
            serde_json::from_str(r#"{"api_version":1,"id":"legacy","name":"老插件"}"#)
                .expect("缺 version 字段必须能解析");
        assert_eq!(m.version, "", "缺省为空串，服务端据此按「有更新」处理");
        m.validate().expect("空版本号必须校验通过");
    }

    /// 填了版本号就得像个版本号，避免上架时才被服务端拒。
    #[test]
    fn version_shape_is_checked_when_present() {
        let mk = |v: &str| -> Manifest {
            serde_json::from_str(&format!(
                r#"{{"api_version":1,"id":"x","name":"x","version":"{v}"}}"#
            ))
            .unwrap()
        };
        for good in ["1.2.0", "0.0.1", "1.2.3-rc.1", "10.20.30"] {
            mk(good)
                .validate()
                .unwrap_or_else(|e| panic!("{good} 应通过：{e}"));
        }
        for bad in ["v1.2.0", "1.2", "latest", "nightly", "-1.0.0"] {
            assert!(mk(bad).validate().is_err(), "{bad} 应被拒");
        }
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
