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

/// 深层去转义：先循环剥外层字符串字面量（最多 8 层，防御恶意的超深嵌套输入），
/// 再把值里内嵌的 JSON object/array 字符串递归展开成真正的结构。
///
/// 典型场景：日志里的 JSON 被转义过两次，或者某个字段的值本身又是一段
/// 序列化后的 JSON —— 一次点击全部还原，不用反复「去转义 → 格式化」。
pub fn unescape_deep(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    // 第一层沿用 unescape 的宽容逻辑。区别在于：不带引号且补引号也解析不了的
    // 输入（比如本来就是普通 JSON），不算失败 —— 原样进入第二步展开内嵌字符串。
    let mut text = if trimmed.starts_with('"') {
        unescape(trimmed)?
    } else {
        unescape(trimmed).unwrap_or_else(|_| trimmed.to_string())
    };
    // 继续剥：只要结果整体仍是一个 JSON 字符串字面量就再剥一层。
    for _ in 1..8 {
        let t = text.trim();
        if !t.starts_with('"') {
            break;
        }
        match serde_json::from_str::<String>(t) {
            Ok(inner) => text = inner,
            Err(_) => break,
        }
    }
    // 第二步：整体是合法 JSON 时展开内嵌的 object/array 字符串。
    // 没发生替换就原样返回，保持既有单层行为（非 JSON 文本剥一层引号也能用）。
    if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
        if expand_embedded_json(&mut value, 8) {
            return serde_json::to_string(&value).map_err(|e| e.to_string());
        }
    }
    Ok(text)
}

/// 递归把 Value 里「看起来是 JSON object/array 的字符串」解析成真正的结构。
/// 只转换 `{` / `[` 开头的字符串 —— "123" / "true" 这类标量字符串必须原样
/// 保留，否则会悄悄改写用户数据的类型。返回是否发生过替换。
///
/// `depth` 只在**剥开一层字符串**时递减：它防的是「字符串套字符串」的恶意
/// 套娃输入。沿 object/array 结构下钻不消耗深度 —— 结构层数由 serde 解析时
/// 已经限制过（默认递归上限 128），正常业务 JSON 嵌套超过 8 层很常见，
/// 不能因为位置深就漏掉里面的转义字段。
fn expand_embedded_json(value: &mut Value, depth: u32) -> bool {
    match value {
        Value::String(s) => {
            if depth == 0 {
                return false;
            }
            let t = s.trim();
            if !(t.starts_with('{') || t.starts_with('[')) {
                return false;
            }
            match serde_json::from_str::<Value>(t) {
                Ok(mut parsed) if parsed.is_object() || parsed.is_array() => {
                    expand_embedded_json(&mut parsed, depth - 1);
                    *value = parsed;
                    true
                }
                _ => false,
            }
        }
        Value::Object(map) => {
            let mut changed = false;
            for (_, v) in map.iter_mut() {
                changed |= expand_embedded_json(v, depth);
            }
            changed
        }
        Value::Array(arr) => {
            let mut changed = false;
            for v in arr.iter_mut() {
                changed |= expand_embedded_json(v, depth);
            }
            changed
        }
        _ => false,
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

    #[test]
    fn unescape_deep_strips_double_escape_in_one_go() {
        let src = r#"{"a":1}"#;
        let esc2 = escape(&escape(src));
        assert_eq!(unescape_deep(&esc2).unwrap(), src);
    }

    #[test]
    fn unescape_deep_expands_nested_json_string_field() {
        // a 字段的值是一段序列化过的 JSON：{"a":"{\"b\":1}"}，再整体转义一层。
        let src = r#"{"a":"{\"b\":1}"}"#;
        let out = unescape_deep(&escape(src)).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["a"].is_object(), "a 字段应展开为真对象：{out}");
        assert_eq!(v["a"]["b"], Value::from(1));
    }

    #[test]
    fn unescape_deep_keeps_scalar_strings_untouched() {
        // "123" / "true" 是用户的字符串数据，绝不能被转成数字 / 布尔。
        let src = r#"{"n":"123","t":"true","o":"{\"x\":2}"}"#;
        let out = unescape_deep(src).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["n"], Value::from("123"));
        assert_eq!(v["t"], Value::from("true"));
        assert!(v["o"].is_object(), "只有 object/array 字符串才展开");
    }

    #[test]
    fn unescape_deep_expands_plain_json_without_outer_quotes() {
        // 没有外层引号的普通 JSON，内嵌的转义 JSON 字符串字段照样能展开。
        let src = r#"{"a":"{\"b\":1}"}"#;
        let out = unescape_deep(src).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"]["b"], Value::from(1));
    }

    #[test]
    fn unescape_deep_still_handles_non_json_text() {
        // 既有单层行为不能丢：非 JSON 文本剥一层引号也要能用。
        assert_eq!(unescape_deep(r#""line1\nline2""#).unwrap(), "line1\nline2");
    }

    #[test]
    fn unescape_deep_reaches_fields_below_eight_structural_levels() {
        // 结构嵌套不消耗防套娃深度：埋在 10 层对象下的转义字段照样展开。
        let mut src = r#"{"payload":"{\"x\":1}"}"#.to_owned();
        for i in 0..10 {
            src = format!(r#"{{"l{i}":{src}}}"#);
        }
        let out = unescape_deep(&src).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let mut cur = &v;
        for i in (0..10).rev() {
            cur = &cur[&format!("l{i}")];
        }
        assert!(cur["payload"].is_object(), "深层字段没展开：{out}");
        assert_eq!(cur["payload"]["x"], Value::from(1));
    }
}
