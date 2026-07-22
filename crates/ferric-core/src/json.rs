//! JSON 工具：格式化 / 压缩 / 校验 / 转义 / 去转义 / 排序键。

use serde::{Deserialize, Serialize};
use serde_json::{ser::PrettyFormatter, Serializer, Value};

/// 缩进风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Indent {
    Two,
    Four,
    Tab,
}

impl Indent {
    fn bytes(self) -> &'static [u8] {
        match self {
            Indent::Two => b"  ",
            Indent::Four => b"    ",
            Indent::Tab => b"\t",
        }
    }
}

/// 解析 JSON，成功返回 [`Value`]，失败返回带行列信息的错误串。
pub fn parse(input: &str) -> Result<Value, String> {
    serde_json::from_str::<Value>(input).map_err(|e| e.to_string())
}

/// 仅校验语法。
pub fn validate(input: &str) -> Result<(), String> {
    parse(input).map(|_| ())
}

/// 格式化 / 美化，可选缩进与键名排序。
pub fn format(input: &str, indent: Indent, sort_keys: bool) -> Result<String, String> {
    let mut value = parse(input)?;
    if sort_keys {
        sort_value_keys(&mut value);
    }
    write_pretty(&value, indent)
}

/// 压缩为单行。
pub fn minify(input: &str) -> Result<String, String> {
    let value = parse(input)?;
    serde_json::to_string(&value).map_err(|e| e.to_string())
}

/// 把整段文本转义为一个 JSON 字符串字面量（含首尾引号）。
pub fn escape(input: &str) -> String {
    Value::String(input.to_string()).to_string()
}

/// 去转义：把一个 JSON 字符串字面量还原为原始文本。
/// 若输入本身不带引号，则自动补上再解析，尽量宽容。
pub fn unescape(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let candidate = if trimmed.starts_with('"') {
        trimmed.to_string()
    } else {
        format!("\"{trimmed}\"")
    };
    match serde_json::from_str::<String>(&candidate) {
        Ok(s) => Ok(s),
        Err(e) => Err(e.to_string()),
    }
}

/// 递归排序对象键（字典序）。
pub fn sort_value_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // serde_json 默认 Map 保序；转成有序集合后重建。
            let mut entries: Vec<(String, Value)> =
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            map.clear();
            for (k, mut v) in entries {
                sort_value_keys(&mut v);
                map.insert(k, v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                sort_value_keys(v);
            }
        }
        _ => {}
    }
}

fn write_pretty(value: &Value, indent: Indent) -> Result<String, String> {
    let mut buf = Vec::new();
    let formatter = PrettyFormatter::with_indent(indent.bytes());
    let mut ser = Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut ser).map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_then_minify_roundtrips() {
        let src = r#"{"b":1,"a":[1,2,3]}"#;
        let pretty = format(src, Indent::Two, false).unwrap();
        assert!(pretty.contains('\n'));
        let min = minify(&pretty).unwrap();
        assert_eq!(min, r#"{"b":1,"a":[1,2,3]}"#);
    }

    #[test]
    fn sort_keys_orders_alphabetically() {
        let out = format(r#"{"b":1,"a":2}"#, Indent::Two, true).unwrap();
        let a = out.find("\"a\"").unwrap();
        let b = out.find("\"b\"").unwrap();
        assert!(a < b);
    }

    #[test]
    fn escape_unescape_roundtrip() {
        let s = "line1\n\"quoted\"\ttab";
        let esc = escape(s);
        assert!(esc.starts_with('"') && esc.ends_with('"'));
        let back = unescape(&esc).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn validate_rejects_bad_json() {
        assert!(validate("{ not json }").is_err());
        assert!(validate(r#"{"ok":true}"#).is_ok());
    }
}
