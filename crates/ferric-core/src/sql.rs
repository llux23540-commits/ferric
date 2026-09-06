//! SQL 格式化 / 压缩。

use sqlformat::{FormatOptions, Indent, QueryParams};

/// 关键字大小写。
///
/// 只有这两档：`sqlformat` 自带的「保持原样」看着像个开关，实际上是**单向**的
/// —— 一旦排过一次变成大写，再关掉它也只是「保持（已经变成大写的）原样」，
/// 界面上按下去毫无变化（用户报的就是「关键字大写没啥用」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Upper,
    Lower,
}

/// 美化 SQL：关键字换行缩进，关键字按 `case` 统一大小写。
pub fn format(sql: &str, case: Case) -> String {
    let upper = run(sql, true);
    match case {
        Case::Upper => upper,
        Case::Lower => lower_keywords(sql, &upper),
    }
}

fn run(sql: &str, uppercase: bool) -> String {
    sqlformat::format(
        sql,
        &QueryParams::None,
        FormatOptions {
            indent: Indent::Spaces(2),
            uppercase,
            lines_between_queries: 1,
        },
    )
}

/// 把 `upper`（关键字已大写的排版结果）里**属于关键字**的字符改成小写。
///
/// 关键字位置不靠自备词表：词表迟早与 sqlformat 的认定漂移，而且多词关键字
/// （`ORDER BY` / `LEFT OUTER JOIN`）还得跟着抄一份。改成问 sqlformat 自己 ——
/// 把输入整体 ASCII 小写后排两遍（一遍不动大小写、一遍关键字大写），
/// 两份**只在关键字字符上**不同，这就是掩码。
///
/// 三份输出逐字节对齐的前提：ASCII 小写不改变字节数，而 sqlformat 的分词与
/// 排版判断都是大小写无关的，因此 token 边界与换行位置完全一致。
/// 万一对不齐（上游改了行为），宁可整份保持大写也不去猜 ——
/// 猜错会动到标识符或字符串字面量里的内容。
fn lower_keywords(sql: &str, upper: &str) -> String {
    let lowered = sql.to_ascii_lowercase();
    let plain = run(&lowered, false);
    let marked = run(&lowered, true);
    if plain.len() != marked.len() || marked.len() != upper.len() {
        return upper.to_owned();
    }
    let bytes: Vec<u8> = upper
        .bytes()
        .zip(plain.bytes())
        .zip(marked.bytes())
        .map(|((u, p), m)| if p == m { u } else { u.to_ascii_lowercase() })
        .collect();
    // 只改过 ASCII 字母的大小写，非 ASCII 字节原样透传 —— UTF-8 仍然合法。
    String::from_utf8(bytes).unwrap_or_else(|_| upper.to_owned())
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
        let out = format("select a,b from t where x=1", Case::Upper);
        assert!(out.contains("SELECT"));
        assert!(out.contains("FROM"));
        assert!(out.contains('\n'));
    }

    #[test]
    fn lower_case_puts_keywords_back_to_lowercase() {
        // 关掉大写必须真的能改回来 —— 这是这个开关唯一的用处。
        let out = format("SELECT A,B FROM T WHERE X=1 ORDER BY A", Case::Lower);
        assert!(out.contains("select"), "{out}");
        assert!(out.contains("from"), "{out}");
        assert!(out.contains("where"), "{out}");
        // 多词关键字整条都要跟着变（词表方案最容易漏的就是这种）
        assert!(out.contains("order by"), "{out}");
        assert!(!out.contains("SELECT"), "{out}");
    }

    #[test]
    fn lower_case_leaves_identifiers_and_literals_alone() {
        // 只碰关键字：标识符是用户写的名字，字符串字面量更是数据。
        let out = format(
            "select Id, Name from UserTable where Note = 'FROM Bob AND SELECT'",
            Case::Lower,
        );
        assert!(out.contains("Id"), "{out}");
        assert!(out.contains("UserTable"), "{out}");
        assert!(out.contains("'FROM Bob AND SELECT'"), "{out}");
    }

    #[test]
    fn the_two_cases_differ_only_by_case() {
        // 两档排版必须完全一致，只有字母大小写不同 ——
        // 否则「切换大小写」会顺带改动布局。
        let sql = "select a, b from t left join u on u.id = t.id group by a having count(*) > 1";
        let up = format(sql, Case::Upper);
        let low = format(sql, Case::Lower);
        assert_ne!(up, low);
        assert!(up.eq_ignore_ascii_case(&low), "up={up}\nlow={low}");
    }

    #[test]
    fn non_ascii_identifiers_survive_the_lowercase_pass() {
        // 掩码是逐字节做的，中文列名不能被截断成乱码。
        let out = format(
            "SELECT 城市, 人口 FROM 城市表 WHERE 人口 > 100",
            Case::Lower,
        );
        assert!(out.contains("城市表"), "{out}");
        assert!(out.contains("人口"), "{out}");
        assert!(out.contains("select"), "{out}");
    }

    #[test]
    fn minify_single_line() {
        let out = minify("select   a,\n  b\nfrom t");
        assert_eq!(out, "select a, b from t");
        assert!(!out.contains('\n'));
    }
}
