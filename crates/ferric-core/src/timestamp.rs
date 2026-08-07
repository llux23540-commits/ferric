//! 时间戳互转：Unix 时间戳 ↔ 指定时区的日期时间。

use chrono::{Datelike, Local, LocalResult, NaiveDate, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// 时间戳精度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Precision {
    Seconds,
    Millis,
}

/// 当前 Unix 时间戳。
pub fn now(precision: Precision) -> i64 {
    let now = Utc::now();
    match precision {
        Precision::Seconds => now.timestamp(),
        Precision::Millis => now.timestamp_millis(),
    }
}

/// 时间戳 → 指定时区的格式化字符串（`YYYY-MM-DD HH:MM:SS`）。
pub fn to_datetime(ts: i64, precision: Precision, tz: Tz) -> Result<String, String> {
    let dt_utc = match precision {
        Precision::Seconds => Utc.timestamp_opt(ts, 0),
        Precision::Millis => Utc.timestamp_millis_opt(ts),
    };
    match dt_utc {
        LocalResult::Single(dt) => Ok(dt
            .with_timezone(&tz)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()),
        _ => Err("无效的时间戳".into()),
    }
}

/// 逐项日期时间（在指定时区）→ Unix 时间戳（秒级）。
#[allow(clippy::too_many_arguments)]
pub fn parts_to_unix(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
    tz: Tz,
) -> Result<i64, String> {
    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or("无效的年月日")?;
    let naive = date.and_hms_opt(hour, min, sec).ok_or("无效的时分秒")?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Ok(dt.timestamp()),
        LocalResult::Ambiguous(dt, _) => Ok(dt.timestamp()),
        LocalResult::None => Err("该时区下不存在此时间（如夏令时跳变）".into()),
    }
}

/// 快速输入格式 `YYYYMMDDHHMMSS`（14 位）→ Unix 时间戳（秒级）。
pub fn parse_compact(input: &str, tz: Tz) -> Result<i64, String> {
    let s: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    if s.len() != 14 {
        return Err("需要 14 位数字：YYYYMMDDHHMMSS".into());
    }
    let year: i32 = s[0..4].parse().map_err(|_| "年份解析失败")?;
    let month: u32 = s[4..6].parse().map_err(|_| "月份解析失败")?;
    let day: u32 = s[6..8].parse().map_err(|_| "日期解析失败")?;
    let hour: u32 = s[8..10].parse().map_err(|_| "小时解析失败")?;
    let min: u32 = s[10..12].parse().map_err(|_| "分钟解析失败")?;
    let sec: u32 = s[12..14].parse().map_err(|_| "秒解析失败")?;
    parts_to_unix(year, month, day, hour, min, sec, tz)
}

/// 灵活日期解析 → Unix 时间戳（秒级，指定时区）。
///
/// 接受：`2025-07-08 12:03:05`、`2025/7/8 12:03`、`2025-07-08`、`2025/7/8`、
/// `20250708120305`（14 位）、`20250708`（8 位）。时间部分可省略（默认 00:00:00）。
pub fn parse_flexible(input: &str, tz: Tz) -> Result<i64, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("请输入日期时间".into());
    }
    // 纯数字：14 位 compact / 8 位 YYYYMMDD。
    if t.chars().all(|c| c.is_ascii_digit()) {
        if t.len() == 14 {
            return parse_compact(t, tz);
        }
        if t.len() == 8 {
            let y = t[0..4].parse().map_err(|_| "年份解析失败")?;
            let mo = t[4..6].parse().map_err(|_| "月份解析失败")?;
            let d = t[6..8].parse().map_err(|_| "日期解析失败")?;
            return parts_to_unix(y, mo, d, 0, 0, 0, tz);
        }
        return Err("请输入形如 2025-07-08 12:03:05，或 14 位 YYYYMMDDHHMMSS".into());
    }
    // 归一分隔符，拆日期 / 时间。
    let norm = t.replace('/', "-").replace('T', " ");
    let mut it = norm.split_whitespace();
    let date_part = it.next().ok_or("缺少日期")?;
    let time_part = it.next();

    let ds: Vec<&str> = date_part.split('-').collect();
    if ds.len() != 3 {
        return Err("日期需为 年-月-日".into());
    }
    let y: i32 = ds[0].parse().map_err(|_| "年份解析失败")?;
    let mo: u32 = ds[1].parse().map_err(|_| "月份解析失败")?;
    let d: u32 = ds[2].parse().map_err(|_| "日期解析失败")?;

    let (mut h, mut mi, mut s) = (0u32, 0u32, 0u32);
    if let Some(tp) = time_part {
        let ts: Vec<&str> = tp.split(':').collect();
        if !ts.is_empty() {
            h = ts[0].parse().map_err(|_| "小时解析失败")?;
        }
        if ts.len() >= 2 {
            mi = ts[1].parse().map_err(|_| "分钟解析失败")?;
        }
        if ts.len() >= 3 {
            s = ts[2].parse().map_err(|_| "秒解析失败")?;
        }
    }
    parts_to_unix(y, mo, d, h, mi, s, tz)
}

/// 当前系统时区的 UTC 偏移，如 `UTC+08:00`。
pub fn system_offset() -> String {
    let secs = Local::now().offset().fix().local_minus_utc();
    let sign = if secs >= 0 { '+' } else { '-' };
    let a = secs.abs();
    format!("UTC{}{:02}:{:02}", sign, a / 3600, (a % 3600) / 60)
}

/// 把时间戳拆成时区内的 (年,月,日,时,分,秒)，用于回填逐项输入框。
pub fn to_parts(
    ts: i64,
    precision: Precision,
    tz: Tz,
) -> Result<(i32, u32, u32, u32, u32, u32), String> {
    let dt_utc = match precision {
        Precision::Seconds => Utc.timestamp_opt(ts, 0),
        Precision::Millis => Utc.timestamp_millis_opt(ts),
    };
    match dt_utc {
        LocalResult::Single(dt) => {
            let l = dt.with_timezone(&tz);
            Ok((
                l.year(),
                l.month(),
                l.day(),
                l.hour(),
                l.minute(),
                l.second(),
            ))
        }
        _ => Err("无效的时间戳".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Asia::Shanghai;

    #[test]
    fn parts_roundtrip_shanghai() {
        // 2025-07-08 12:03:05 北京时间
        let ts = parts_to_unix(2025, 7, 8, 12, 3, 5, Shanghai).unwrap();
        let (y, mo, d, h, mi, s) = to_parts(ts, Precision::Seconds, Shanghai).unwrap();
        assert_eq!((y, mo, d, h, mi, s), (2025, 7, 8, 12, 3, 5));
    }

    #[test]
    fn compact_parses() {
        let ts = parse_compact("20250708120305", Shanghai).unwrap();
        let s = to_datetime(ts, Precision::Seconds, Shanghai).unwrap();
        assert_eq!(s, "2025-07-08 12:03:05");
    }

    #[test]
    fn compact_rejects_wrong_length() {
        assert!(parse_compact("2025", Shanghai).is_err());
    }

    #[test]
    fn flexible_formats() {
        let want = parse_compact("20250708120305", Shanghai).unwrap();
        assert_eq!(
            parse_flexible("2025-07-08 12:03:05", Shanghai).unwrap(),
            want
        );
        assert_eq!(parse_flexible("2025/7/8 12:03:05", Shanghai).unwrap(), want);
        assert_eq!(parse_flexible("20250708120305", Shanghai).unwrap(), want);
        // 仅日期 → 00:00:00
        let day = parse_flexible("2025-07-08", Shanghai).unwrap();
        assert_eq!(
            to_datetime(day, Precision::Seconds, Shanghai).unwrap(),
            "2025-07-08 00:00:00"
        );
    }
}

// ============================ 时区中文检索 ============================

/// 常用时区的中文别名。
///
/// `chrono_tz` 给的名字全是英文（`Asia/Shanghai`），中文用户想找「上海」「北京」
/// 是搜不到的。这里给常用时区配上中文城市名 / 国家名 / 惯用叫法，做成一串以空格
/// 分隔的关键词 —— 首个词用于展示，全部词参与匹配。
///
/// 只覆盖常用时区（约 590 个里的一百多个）：其余仍可用英文名搜到，不影响使用。
const ZH_ALIASES: &[(&str, &str)] = &[
    // —— 中国与周边
    ("Asia/Shanghai", "上海 北京 中国 中国标准时间 东八区"),
    ("Asia/Chongqing", "重庆 中国"),
    ("Asia/Urumqi", "乌鲁木齐 新疆 中国"),
    ("Asia/Harbin", "哈尔滨 中国"),
    ("Asia/Hong_Kong", "香港 中国 港"),
    ("Asia/Macau", "澳门 中国 澳"),
    ("Asia/Taipei", "台北 台湾 中国"),
    ("Asia/Tokyo", "东京 日本 日"),
    ("Asia/Seoul", "首尔 韩国 汉城 韩"),
    ("Asia/Pyongyang", "平壤 朝鲜"),
    ("Asia/Ulaanbaatar", "乌兰巴托 蒙古"),
    // —— 东南亚 / 南亚
    ("Asia/Singapore", "新加坡 狮城"),
    ("Asia/Kuala_Lumpur", "吉隆坡 马来西亚"),
    ("Asia/Bangkok", "曼谷 泰国"),
    ("Asia/Jakarta", "雅加达 印尼 印度尼西亚"),
    ("Asia/Manila", "马尼拉 菲律宾"),
    ("Asia/Ho_Chi_Minh", "胡志明 西贡 河内 越南"),
    ("Asia/Phnom_Penh", "金边 柬埔寨"),
    ("Asia/Vientiane", "万象 老挝"),
    ("Asia/Yangon", "仰光 缅甸"),
    ("Asia/Kolkata", "加尔各答 新德里 印度 孟买"),
    ("Asia/Colombo", "科伦坡 斯里兰卡"),
    ("Asia/Kathmandu", "加德满都 尼泊尔"),
    ("Asia/Dhaka", "达卡 孟加拉"),
    ("Asia/Karachi", "卡拉奇 巴基斯坦"),
    ("Asia/Kabul", "喀布尔 阿富汗"),
    // —— 中东 / 中亚
    ("Asia/Dubai", "迪拜 阿联酋 阿布扎比"),
    ("Asia/Riyadh", "利雅得 沙特"),
    ("Asia/Tehran", "德黑兰 伊朗"),
    ("Asia/Baghdad", "巴格达 伊拉克"),
    ("Asia/Jerusalem", "耶路撒冷 以色列"),
    ("Asia/Istanbul", "伊斯坦布尔 土耳其"),
    ("Asia/Tashkent", "塔什干 乌兹别克斯坦"),
    ("Asia/Almaty", "阿拉木图 哈萨克斯坦"),
    ("Asia/Baku", "巴库 阿塞拜疆"),
    ("Asia/Yerevan", "埃里温 亚美尼亚"),
    ("Asia/Tbilisi", "第比利斯 格鲁吉亚"),
    // —— 俄罗斯
    ("Europe/Moscow", "莫斯科 俄罗斯 俄"),
    ("Asia/Yekaterinburg", "叶卡捷琳堡 俄罗斯"),
    ("Asia/Novosibirsk", "新西伯利亚 俄罗斯"),
    ("Asia/Vladivostok", "海参崴 符拉迪沃斯托克 俄罗斯"),
    // —— 欧洲
    ("Europe/London", "伦敦 英国 英"),
    ("Europe/Dublin", "都柏林 爱尔兰"),
    ("Europe/Paris", "巴黎 法国 法"),
    ("Europe/Berlin", "柏林 德国 德"),
    ("Europe/Madrid", "马德里 西班牙"),
    ("Europe/Lisbon", "里斯本 葡萄牙"),
    ("Europe/Rome", "罗马 意大利 意"),
    ("Europe/Amsterdam", "阿姆斯特丹 荷兰"),
    ("Europe/Brussels", "布鲁塞尔 比利时"),
    ("Europe/Zurich", "苏黎世 瑞士"),
    ("Europe/Vienna", "维也纳 奥地利"),
    ("Europe/Stockholm", "斯德哥尔摩 瑞典"),
    ("Europe/Oslo", "奥斯陆 挪威"),
    ("Europe/Copenhagen", "哥本哈根 丹麦"),
    ("Europe/Helsinki", "赫尔辛基 芬兰"),
    ("Europe/Warsaw", "华沙 波兰"),
    ("Europe/Prague", "布拉格 捷克"),
    ("Europe/Budapest", "布达佩斯 匈牙利"),
    ("Europe/Athens", "雅典 希腊"),
    ("Europe/Bucharest", "布加勒斯特 罗马尼亚"),
    ("Europe/Kyiv", "基辅 乌克兰"),
    ("Europe/Minsk", "明斯克 白俄罗斯"),
    // —— 北美
    ("America/New_York", "纽约 美国 美东 东部时间"),
    ("America/Chicago", "芝加哥 美国 中部时间"),
    ("America/Denver", "丹佛 美国 山地时间"),
    ("America/Phoenix", "凤凰城 美国 亚利桑那"),
    (
        "America/Los_Angeles",
        "洛杉矶 美国 美西 太平洋时间 旧金山 硅谷 西雅图",
    ),
    ("America/Anchorage", "安克雷奇 阿拉斯加 美国"),
    ("Pacific/Honolulu", "檀香山 夏威夷 美国"),
    ("America/Toronto", "多伦多 加拿大"),
    ("America/Vancouver", "温哥华 加拿大"),
    ("America/Montreal", "蒙特利尔 加拿大"),
    ("America/Mexico_City", "墨西哥城 墨西哥"),
    // —— 中南美
    ("America/Sao_Paulo", "圣保罗 巴西"),
    ("America/Argentina/Buenos_Aires", "布宜诺斯艾利斯 阿根廷"),
    ("America/Santiago", "圣地亚哥 智利"),
    ("America/Bogota", "波哥大 哥伦比亚"),
    ("America/Lima", "利马 秘鲁"),
    ("America/Caracas", "加拉加斯 委内瑞拉"),
    ("America/Havana", "哈瓦那 古巴"),
    // —— 非洲
    ("Africa/Cairo", "开罗 埃及"),
    ("Africa/Johannesburg", "约翰内斯堡 南非"),
    ("Africa/Lagos", "拉各斯 尼日利亚"),
    ("Africa/Nairobi", "内罗毕 肯尼亚"),
    ("Africa/Casablanca", "卡萨布兰卡 摩洛哥"),
    ("Africa/Accra", "阿克拉 加纳"),
    ("Africa/Addis_Ababa", "亚的斯亚贝巴 埃塞俄比亚"),
    // —— 大洋洲
    ("Australia/Sydney", "悉尼 澳大利亚 澳洲"),
    ("Australia/Melbourne", "墨尔本 澳大利亚 澳洲"),
    ("Australia/Brisbane", "布里斯班 澳大利亚 澳洲"),
    ("Australia/Perth", "珀斯 澳大利亚 澳洲"),
    ("Australia/Adelaide", "阿德莱德 澳大利亚 澳洲"),
    ("Australia/Darwin", "达尔文 澳大利亚 澳洲"),
    ("Pacific/Auckland", "奥克兰 新西兰"),
    ("Pacific/Fiji", "斐济"),
    ("Pacific/Guam", "关岛"),
    // —— 通用
    ("UTC", "协调世界时 世界时 零时区 标准"),
    ("GMT", "格林尼治 格林威治 零时区"),
];

/// 大区前缀的中文名 —— 输入「亚洲」能列出所有 `Asia/*`。
const ZH_REGIONS: &[(&str, &str)] = &[
    ("Asia/", "亚洲 亚"),
    ("Europe/", "欧洲 欧"),
    ("America/", "美洲 美"),
    ("Africa/", "非洲 非"),
    ("Australia/", "澳洲 大洋洲 澳"),
    ("Pacific/", "太平洋"),
    ("Atlantic/", "大西洋"),
    ("Indian/", "印度洋"),
    ("Antarctica/", "南极"),
    ("Arctic/", "北极"),
];

/// 时区的中文显示名（取别名里的第一个词）。没有配中文的返回 `None`。
pub fn zh_name(tz_name: &str) -> Option<&'static str> {
    ZH_ALIASES
        .iter()
        .find(|(n, _)| *n == tz_name)
        .and_then(|(_, zh)| zh.split(' ').next())
}

/// 不分配的「宽松包含」：ASCII 忽略大小写，且 `_` 与空格视为同一个字符。
///
/// 原先是 `haystack.to_lowercase().contains(&query.to_lowercase())` 再加一次
/// `replace('_', " ")` —— 每比一个时区 3 次堆分配，筛一遍 597 个就是近 1800 次。
/// 改成按字节比：时区标识全是 ASCII，`to_ascii_lowercase` 只动 ASCII 字节，
/// 多字节 UTF-8 序列原样保留；UTF-8 又是自同步编码，字节级子串命中必然落在字符
/// 边界上，不会出现「半个汉字匹配上了」这种假阳性。
///
/// 说明一句公道话：纯吞吐上这个朴素扫描并不比标准库的子串搜索快（后者是
/// SIMD 优化过的两路算法，实测一遍 597 个时区 60µs vs 38µs）。换掉它图的是
/// **不分配**；真正省下的时间来自调用方把结果缓存起来，从每帧一遍变成每次改
/// 搜索词才一遍。两者都不是什么大头，别为了它再折腾。
fn loose_contains(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    let norm = |c: u8| {
        if c == b'_' {
            b' '
        } else {
            c.to_ascii_lowercase()
        }
    };
    (0..=h.len() - n.len()).any(|i| (0..n.len()).all(|j| norm(h[i + j]) == norm(n[j])))
}

/// 时区是否匹配搜索词。**中英文都能搜**：
/// 英文比对时区标识本身（不区分大小写），中文比对城市 / 国家 / 大区别名。
pub fn tz_matches(tz_name: &str, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    // 英文：直接比标识（顺带把下划线当空格，"los angeles" 也能搜到）
    if loose_contains(tz_name, q) {
        return true;
    }
    // 中文：城市 / 国家别名
    if let Some((_, zh)) = ZH_ALIASES.iter().find(|(n, _)| *n == tz_name) {
        if zh.split(' ').any(|w| w.contains(q)) {
            return true;
        }
    }
    // 中文：大区（「亚洲」列出所有 Asia/*）
    ZH_REGIONS
        .iter()
        .any(|(prefix, zh)| tz_name.starts_with(prefix) && zh.split(' ').any(|w| w.contains(q)))
}

#[cfg(test)]
mod tz_search_tests {
    use super::*;

    #[test]
    fn english_search_still_works() {
        assert!(tz_matches("Asia/Shanghai", "shang"));
        assert!(tz_matches("Asia/Shanghai", "ASIA"));
        assert!(tz_matches("America/Los_Angeles", "los_angeles"));
        // 下划线当空格也能搜到
        assert!(tz_matches("America/Los_Angeles", "los angeles"));
        assert!(!tz_matches("Asia/Shanghai", "tokyo"));
    }

    /// 用户要求的中文检索：城市、国家、惯用叫法都要能搜。
    #[test]
    fn chinese_search_finds_zones() {
        assert!(tz_matches("Asia/Shanghai", "上海"));
        assert!(tz_matches("Asia/Shanghai", "北京"));
        assert!(tz_matches("Asia/Shanghai", "中国"));
        assert!(tz_matches("Asia/Tokyo", "东京"));
        assert!(tz_matches("Asia/Tokyo", "日本"));
        assert!(tz_matches("America/Los_Angeles", "洛杉矶"));
        assert!(tz_matches("America/Los_Angeles", "美西"));
        assert!(tz_matches("Europe/London", "伦敦"));
        assert!(tz_matches("UTC", "世界时"));
        // 不该误命中
        assert!(!tz_matches("Asia/Tokyo", "上海"));
        assert!(!tz_matches("Europe/Paris", "东京"));
    }

    /// 输入大区名要能列出该大区全部时区。
    #[test]
    fn chinese_region_search() {
        assert!(tz_matches("Asia/Kathmandu", "亚洲"));
        assert!(tz_matches("Europe/Malta", "欧洲"));
        assert!(tz_matches("Africa/Tunis", "非洲"));
        assert!(!tz_matches("Europe/Paris", "亚洲"));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(tz_matches("Asia/Shanghai", ""));
        assert!(tz_matches("Asia/Shanghai", "   "));
    }

    /// 展示用的中文名取别名首词。
    #[test]
    fn zh_display_name() {
        assert_eq!(zh_name("Asia/Shanghai"), Some("上海"));
        assert_eq!(zh_name("Europe/Paris"), Some("巴黎"));
        assert_eq!(zh_name("Asia/Kathmandu"), Some("加德满都"));
        assert_eq!(zh_name("Europe/Malta"), None); // 未配中文的仍可用英文搜
    }

    /// 改写成不分配的字节比较后，语义必须与原先的
    /// `to_lowercase().contains()` + `replace('_', " ")` 写法**逐条一致**。
    ///
    /// 拿全部 590 个时区 × 一组覆盖各种形态的查询词做全量对拍：性能优化最怕的是
    /// 「快了，但少匹配了一条」，那种回归靠零星几个断言是抓不到的。
    #[test]
    fn matching_is_identical_to_the_allocating_reference() {
        fn reference(tz_name: &str, query: &str) -> bool {
            let q = query.trim();
            if q.is_empty() {
                return true;
            }
            let lower = tz_name.to_lowercase();
            let ql = q.to_lowercase();
            if lower.contains(&ql) || lower.replace('_', " ").contains(&ql) {
                return true;
            }
            if let Some((_, zh)) = ZH_ALIASES.iter().find(|(n, _)| *n == tz_name) {
                if zh.split(' ').any(|w| w.contains(q)) {
                    return true;
                }
            }
            ZH_REGIONS.iter().any(|(prefix, zh)| {
                tz_name.starts_with(prefix) && zh.split(' ').any(|w| w.contains(q))
            })
        }

        let queries = [
            "",
            "   ",
            "a",
            "A",
            "sha",
            "SHA",
            "ShAnG",
            "asia",
            "ASIA/",
            "/",
            "_",
            " ",
            "los_angeles",
            "los angeles",
            "Los Angeles",
            "new_york",
            "new york",
            "utc",
            "gmt",
            "上海",
            "北京",
            "亚洲",
            "美西",
            "东京",
            "zzzz",
            "shanghai xyz",
            "中国",
        ];
        for z in chrono_tz::TZ_VARIANTS.iter() {
            for q in queries {
                assert_eq!(
                    tz_matches(z.name(), q),
                    reference(z.name(), q),
                    "「{q}」对 {} 的匹配结果与参考实现不一致",
                    z.name()
                );
            }
        }
    }

    /// 下划线与空格互通是**双向**的：查询里写哪一种都该命中。
    #[test]
    fn underscore_and_space_are_interchangeable() {
        assert!(tz_matches("America/Los_Angeles", "los_angeles"));
        assert!(tz_matches("America/Los_Angeles", "los angeles"));
        assert!(tz_matches("America/Port_of_Spain", "port of spain"));
    }

    /// 别名表本身的卫生检查：时区名必须真实存在，否则是打错了字。
    #[test]
    fn alias_table_references_real_zones() {
        let bad: Vec<_> = ZH_ALIASES
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| !chrono_tz::TZ_VARIANTS.iter().any(|z| z.name() == *n))
            .collect();
        assert!(bad.is_empty(), "别名表里这些不是有效时区：{bad:?}");
    }
}
