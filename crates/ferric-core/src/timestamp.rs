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
        LocalResult::Single(dt) => Ok(dt.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S").to_string()),
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
    let naive = date
        .and_hms_opt(hour, min, sec)
        .ok_or("无效的时分秒")?;
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
pub fn to_parts(ts: i64, precision: Precision, tz: Tz) -> Result<(i32, u32, u32, u32, u32, u32), String> {
    let dt_utc = match precision {
        Precision::Seconds => Utc.timestamp_opt(ts, 0),
        Precision::Millis => Utc.timestamp_millis_opt(ts),
    };
    match dt_utc {
        LocalResult::Single(dt) => {
            let l = dt.with_timezone(&tz);
            Ok((l.year(), l.month(), l.day(), l.hour(), l.minute(), l.second()))
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
        assert_eq!(parse_flexible("2025-07-08 12:03:05", Shanghai).unwrap(), want);
        assert_eq!(parse_flexible("2025/7/8 12:03:05", Shanghai).unwrap(), want);
        assert_eq!(parse_flexible("20250708120305", Shanghai).unwrap(), want);
        // 仅日期 → 00:00:00
        let day = parse_flexible("2025-07-08", Shanghai).unwrap();
        assert_eq!(to_datetime(day, Precision::Seconds, Shanghai).unwrap(), "2025-07-08 00:00:00");
    }
}
