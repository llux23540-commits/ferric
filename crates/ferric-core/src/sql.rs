//! SQL 格式化 / 压缩。

use sqlformat::{FormatOptions, Indent, QueryParams};

/// 美化 SQL：关键字换行缩进，可选关键字大写。
pub fn format(sql: &str, uppercase: bool) -> String {
    let opts = FormatOptions {
        indent: Indent::Spaces(2),
        uppercase,
        lines_between_queries: 1,
    };
    sqlformat::format(sql, &QueryParams::None, opts)
}

/// 压缩为单行：折叠所有空白为单个空格。
pub fn minify(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_multiline() {
        let out = format("select a,b from t where x=1", true);
        assert!(out.contains("SELECT"));
        assert!(out.contains("FROM"));
        assert!(out.contains('\n'));
    }

    #[test]
    fn minify_single_line() {
        let out = minify("select   a,\n  b\nfrom t");
        assert_eq!(out, "select a, b from t");
        assert!(!out.contains('\n'));
    }
}
