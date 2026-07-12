//! JSON → YAML 转换。

use serde_json::Value;

/// 把 JSON 文本转换为 YAML。空输入返回空串。
pub fn json_to_yaml(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let value: Value = serde_json::from_str(trimmed).map_err(|e| e.to_string())?;
    serde_yaml::to_string(&value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_object() {
        let y = json_to_yaml(r#"{"name":"ferric","tags":["json","yaml"]}"#).unwrap();
        assert!(y.contains("name: ferric"));
        assert!(y.contains("- json"));
        assert!(y.contains("- yaml"));
    }

    #[test]
    fn empty_ok() {
        assert_eq!(json_to_yaml("   ").unwrap(), "");
    }

    #[test]
    fn invalid_json_errs() {
        assert!(json_to_yaml("{bad").is_err());
    }
}
