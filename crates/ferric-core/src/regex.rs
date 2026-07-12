//! 正则测试（基于 fancy-regex，支持前后瞻等 JS 常见语法）。

use fancy_regex::Regex;

/// 单个匹配。
#[derive(Debug, Clone)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// 捕获分组（1 起）；未参与匹配的分组为 None。
    pub groups: Vec<Option<String>>,
}

/// 用 `pattern` + `flags`（`g i m s x` 的任意组合）在 `text` 上查找匹配。
///
/// 不含 `g` 时只返回首个匹配（与 JS 一致）。
pub fn find_all(pattern: &str, flags: &str, text: &str) -> Result<Vec<Match>, String> {
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    let inline: String = flags.chars().filter(|c| "imsx".contains(*c)).collect();
    let pat = if inline.is_empty() {
        pattern.to_string()
    } else {
        format!("(?{inline}){pattern}")
    };
    let re = Regex::new(&pat).map_err(|e| e.to_string())?;
    let global = flags.contains('g');

    let mut out = Vec::new();
    for caps in re.captures_iter(text) {
        let caps = caps.map_err(|e| e.to_string())?;
        let whole = caps.get(0).expect("group 0 always present");
        let groups = (1..caps.len())
            .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
            .collect();
        out.push(Match {
            start: whole.start(),
            end: whole.end(),
            text: whole.as_str().to_string(),
            groups,
        });
        if !global {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_groups() {
        let m = find_all(r"(\w+)@(\w+\.\w+)", "g", "hi@ferric.dev and a@b.io").unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].text, "hi@ferric.dev");
        assert_eq!(m[0].groups[0].as_deref(), Some("hi"));
        assert_eq!(m[0].groups[1].as_deref(), Some("ferric.dev"));
    }

    #[test]
    fn non_global_first_only() {
        let m = find_all(r"\d+", "", "1 2 3").unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].text, "1");
    }

    #[test]
    fn ignore_case_flag() {
        let m = find_all("abc", "gi", "ABC abc").unwrap();
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn invalid_pattern_errs() {
        assert!(find_all("(", "", "x").is_err());
    }
}
