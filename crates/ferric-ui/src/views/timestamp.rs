//! 时间戳 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `TimestampView`：当前时间戳（可实时刷新）+
//! 可搜索时区 + 双向转换（时间戳 ↔ 日期时间）。换算全在
//! `ferric_core::timestamp`（chrono-tz，597 个时区），没动过。
//!
//! # 实时刷新的代价在 Slint 上小得多
//!
//! egui 时代这里是重灾区：立即模式没有局部重绘，为了让秒数跳动就得整窗重绘，
//! 而软件光栅化的机器上整窗重绘要上百毫秒 —— 所以那一版做了一整套
//! 「对齐秒边界 / 失焦零调度 / 静置 90 秒自动停表」。
//!
//! Slint 是 retained mode：改一个 property 只重画那一小块脏区域。所以这里
//! 只保留「实时刷新」开关本身（用户可能就是不想要跳动的数字），
//! 定时器由外壳按秒驱动，不再需要那套省帧的补偿逻辑。

use crate::icons;
use crate::tool::{Tool, ToolMeta};
use chrono_tz::Tz;
use ferric_core::timestamp::{self, Precision};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct TimestampDraft {
    tz: String,
    ts_input: String,
    date_input: String,
}

/// 时区列表里的一项。
pub struct TzRow {
    pub name: String,
    /// `Asia/Shanghai · 中国标准时间` 这种带中文名的显示文案。
    pub label: String,
}

pub struct TimestampTool {
    pub tz: Tz,
    pub tz_filter: String,
    /// 当前筛选词命中的时区。**缓存**下来，只在筛选词变化时重建 ——
    /// 597 条里每条都要走一次 `tz_matches`（内含堆分配）加一次 `format!`。
    pub tz_hits: Vec<TzRow>,
    tz_hits_key: Option<String>,

    /// 当前时间戳（毫秒）。实时刷新时由外壳的秒定时器推进。
    pub now_ms: i64,
    /// 是否实时刷新。默认**关**：与 egui 版一致 —— 一个一直在跳的数字
    /// 会让人以为界面卡了，而且多数用户是来做一次换算的。
    pub running: bool,

    pub ts_input: String,
    pub ts_output: String,
    pub ts_ok: bool,

    pub date_input: String,
    pub date_output: String,
    pub date_ok: bool,

    /// 系统时区偏移文案（`UTC+08:00`）。
    pub offset: String,
}

impl Default for TimestampTool {
    fn default() -> Self {
        let mut t = Self {
            tz: Tz::Asia__Shanghai,
            tz_filter: String::new(),
            tz_hits: Vec::new(),
            tz_hits_key: None,
            now_ms: timestamp::now(Precision::Millis),
            running: false,
            ts_input: String::new(),
            ts_output: String::new(),
            ts_ok: true,
            date_input: String::new(),
            date_output: String::new(),
            date_ok: true,
            offset: timestamp::system_offset(),
        };
        t.refresh_tz_hits();
        t
    }
}

impl TimestampTool {
    /// 时区列表的显示文案：有中文名的带上。
    fn tz_label(tz: Tz) -> String {
        let name = tz.name();
        match timestamp::zh_name(name) {
            Some(zh) => format!("{name} · {zh}"),
            None => name.to_owned(),
        }
    }

    /// 按当前筛选词重建命中列表。**只在筛选词真的变了时才算** ——
    /// 597 条 × (匹配 + format!) 不该每次同步都跑一遍。
    pub fn refresh_tz_hits(&mut self) {
        if self.tz_hits_key.as_deref() == Some(self.tz_filter.as_str()) {
            return;
        }
        self.tz_hits = chrono_tz::TZ_VARIANTS
            .iter()
            .copied()
            .filter(|z| timestamp::tz_matches(z.name(), &self.tz_filter))
            .map(|z| TzRow {
                name: z.name().to_owned(),
                label: Self::tz_label(z),
            })
            .collect();
        self.tz_hits_key = Some(self.tz_filter.clone());
    }

    pub fn set_filter(&mut self, f: &str) {
        self.tz_filter = f.to_owned();
        self.refresh_tz_hits();
    }

    /// 按时区名选中（下拉列表项点击）。名字对不上就不动 ——
    /// 列表是我们自己生成的，对不上说明有 bug，静默保持原值比 panic 好。
    pub fn select_tz(&mut self, name: &str) {
        // 选完把筛选词清掉：下拉一关，那个词就没有归属了，
        // 留着会让下次打开只剩上次搜过的那几条。
        self.set_filter("");
        if let Ok(tz) = name.parse::<Tz>() {
            self.tz = tz;
            // 换时区后把已有的两个结果按新时区重算，否则界面上留着旧时区的答案。
            if !self.ts_input.trim().is_empty() {
                self.convert_ts();
            }
            if !self.date_input.trim().is_empty() {
                self.convert_date();
            }
        }
    }

    pub fn tz_label_current(&self) -> String {
        Self::tz_label(self.tz)
    }

    /// 秒定时器的钩子。返回 `true` 表示数字变了（调用方据此刷 UI）。
    pub fn tick(&mut self) -> bool {
        if !self.running {
            return false;
        }
        self.now_ms = timestamp::now(Precision::Millis);
        self.offset = timestamp::system_offset();
        true
    }

    pub fn toggle_running(&mut self) {
        self.running = !self.running;
        if self.running {
            self.now_ms = timestamp::now(Precision::Millis);
        }
    }

    /// 手动刷新（实时刷新关掉时用）。
    pub fn refresh_now(&mut self) {
        self.now_ms = timestamp::now(Precision::Millis);
        self.offset = timestamp::system_offset();
    }

    pub fn now_seconds(&self) -> i64 {
        self.now_ms / 1000
    }

    /// 时间戳 → 日期时间。
    ///
    /// 秒与毫秒**按位数自动判断**：10 位当秒、13 位当毫秒。egui 版是让用户
    /// 手动选精度，但那个选择几乎总是能从输入本身看出来，多一次点击没有意义。
    pub fn convert_ts(&mut self) {
        let raw = self.ts_input.trim();
        if raw.is_empty() {
            self.ts_output.clear();
            self.ts_ok = true;
            return;
        }
        let Ok(n) = raw.parse::<i64>() else {
            self.ts_ok = false;
            self.ts_output = "不是整数时间戳".to_owned();
            return;
        };
        // 毫秒级时间戳是 13 位（2001 年之后）。用绝对值判断，负数（1970 前）同理。
        let precision = if n.abs() >= 100_000_000_000 {
            Precision::Millis
        } else {
            Precision::Seconds
        };
        match timestamp::to_datetime(n, precision, self.tz) {
            Ok(s) => {
                let unit = match precision {
                    Precision::Millis => "毫秒",
                    Precision::Seconds => "秒",
                };
                self.ts_output = format!("{s}（按{unit}解析）");
                self.ts_ok = true;
            }
            Err(e) => {
                self.ts_ok = false;
                self.ts_output = e;
            }
        }
    }

    /// 日期时间 → 时间戳。宽松解析（多种常见格式）。
    pub fn convert_date(&mut self) {
        let raw = self.date_input.trim();
        if raw.is_empty() {
            self.date_output.clear();
            self.date_ok = true;
            return;
        }
        match timestamp::parse_flexible(raw, self.tz) {
            Ok(secs) => {
                self.date_output = format!("{secs}（秒） · {}（毫秒）", secs * 1000);
                self.date_ok = true;
            }
            Err(e) => {
                self.date_ok = false;
                self.date_output = e;
            }
        }
    }

    pub fn set_ts_input(&mut self, v: &str) {
        self.ts_input = v.to_owned();
        self.convert_ts();
    }

    pub fn set_date_input(&mut self, v: &str) {
        self.date_input = v.to_owned();
        self.convert_date();
    }

    /// 把当前时间戳填进输入框并转换（最常用的一个动作）。
    pub fn use_now(&mut self) {
        self.ts_input = self.now_seconds().to_string();
        self.convert_ts();
    }
}

impl Tool for TimestampTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "timestamp",
            name: "时间戳",
            desc: "Unix ↔ 日期时间，秒/毫秒自动识别，全量时区可搜索。",
            icon: icons::CLOCK,
            group: "转换",
            keywords: &["timestamp", "unix", "时间戳", "时间", "date", "时区"],
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&TimestampDraft {
            tz: self.tz.name().to_owned(),
            ts_input: self.ts_input.clone(),
            date_input: self.date_input.clone(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<TimestampDraft>(data) {
            // 时区名来自可写文件，认不出来就保持默认（而不是 panic）。
            if let Ok(tz) = d.tz.parse::<Tz>() {
                self.tz = tz;
            }
            self.ts_input = d.ts_input;
            self.date_input = d.date_input;
            self.convert_ts();
            self.convert_date();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_and_millis_are_told_apart_by_digit_count() {
        // 让用户手选精度是多余的一次点击：位数已经说明了一切。
        let mut t = TimestampTool::default();

        t.set_ts_input("1700000000"); // 10 位 = 秒
        assert!(t.ts_ok, "{}", t.ts_output);
        assert!(t.ts_output.contains("按秒解析"), "{}", t.ts_output);
        assert!(t.ts_output.contains("2023"), "{}", t.ts_output);

        t.set_ts_input("1700000000000"); // 13 位 = 毫秒
        assert!(t.ts_ok, "{}", t.ts_output);
        assert!(t.ts_output.contains("按毫秒解析"), "{}", t.ts_output);
        assert!(t.ts_output.contains("2023"), "同一时刻应当解析成同一年");
    }

    #[test]
    fn non_numeric_input_is_reported_not_panicking() {
        let mut t = TimestampTool::default();
        t.set_ts_input("not a number");
        assert!(!t.ts_ok);
        assert!(t.ts_output.contains("不是整数"));
    }

    #[test]
    fn empty_input_clears_output_without_error() {
        let mut t = TimestampTool::default();
        t.set_ts_input("1700000000");
        assert!(!t.ts_output.is_empty());
        t.set_ts_input("   ");
        assert!(t.ts_output.is_empty(), "空输入该清空结果而不是报错");
        assert!(t.ts_ok);
    }

    #[test]
    fn date_to_timestamp_round_trips() {
        let mut t = TimestampTool::default();
        t.select_tz("UTC");
        t.set_date_input("2023-11-14 22:13:20");
        assert!(t.date_ok, "{}", t.date_output);
        // 该时刻在 UTC 下正是 1700000000
        assert!(
            t.date_output.starts_with("1700000000"),
            "换算结果不对：{}",
            t.date_output
        );
    }

    #[test]
    fn changing_timezone_recomputes_existing_results() {
        // 不重算的话界面上会留着上一个时区的答案，而标签已经变了。
        let mut t = TimestampTool::default();
        t.select_tz("UTC");
        t.set_ts_input("1700000000");
        let utc = t.ts_output.clone();
        t.select_tz("Asia/Tokyo");
        assert_ne!(t.ts_output, utc, "换时区后结果没跟着重算");
    }

    #[test]
    fn unknown_timezone_name_is_ignored() {
        let mut t = TimestampTool::default();
        let before = t.tz;
        t.select_tz("Mars/Olympus_Mons");
        assert_eq!(t.tz, before, "认不出的时区名该忽略，不该 panic");
    }

    #[test]
    fn picking_a_timezone_resets_the_filter() {
        // 下拉是「打开 → 搜 → 选」：选完那个筛选词就没有归属了。
        // 不清的话下次打开只剩上次搜过的那几条，看着像列表丢了。
        let mut t = TimestampTool::default();
        let all = t.tz_hits.len();
        t.set_filter("shanghai");
        assert!(t.tz_hits.len() < all);

        t.select_tz("Asia/Shanghai");
        assert_eq!(t.tz_filter, "");
        assert_eq!(t.tz_hits.len(), all, "选完应当回到全量列表");
        assert!(t.tz_label_current().contains("Shanghai"));
    }

    #[test]
    fn timezone_filter_narrows_the_list_and_is_cached() {
        let mut t = TimestampTool::default();
        let all = t.tz_hits.len();
        assert!(all > 500, "全量时区应当有几百条，实际 {all}");

        t.set_filter("shanghai");
        assert!(t.tz_hits.len() < all);
        assert!(
            t.tz_hits.iter().any(|r| r.name == "Asia/Shanghai"),
            "搜 shanghai 应当命中上海"
        );

        // 缓存：同一个筛选词再来一次不该重建（这里验证结果稳定即可）
        let n = t.tz_hits.len();
        t.refresh_tz_hits();
        assert_eq!(t.tz_hits.len(), n);

        t.set_filter("");
        assert_eq!(t.tz_hits.len(), all, "清空筛选应当回到全量");
    }

    #[test]
    fn chinese_names_are_searchable() {
        // 中文用户会直接搜「上海」。
        let mut t = TimestampTool::default();
        t.set_filter("上海");
        assert!(
            t.tz_hits.iter().any(|r| r.name == "Asia/Shanghai"),
            "中文名搜不到上海"
        );
    }

    #[test]
    fn tick_only_advances_when_running() {
        let mut t = TimestampTool::default();
        assert!(!t.running, "默认不该实时刷新 —— 一直跳的数字像卡住了");
        assert!(!t.tick(), "没开实时刷新时 tick 不该报告变化");

        t.toggle_running();
        assert!(t.running);
        assert!(t.tick());
    }

    #[test]
    fn use_now_fills_the_input_in_seconds() {
        let mut t = TimestampTool::default();
        t.use_now();
        assert_eq!(t.ts_input, t.now_seconds().to_string());
        assert!(t.ts_ok, "{}", t.ts_output);
    }

    #[test]
    fn draft_roundtrip_preserves_tz_and_inputs() {
        let mut t = TimestampTool::default();
        t.select_tz("America/New_York");
        t.set_ts_input("1700000000");
        t.set_date_input("2020-01-02 03:04:05");
        let saved = t.save_draft().expect("必须持久化草稿");

        let mut r = TimestampTool::default();
        r.load_draft(&saved);
        assert_eq!(r.tz.name(), "America/New_York");
        assert_eq!(r.ts_input, "1700000000");
        assert_eq!(r.date_input, "2020-01-02 03:04:05");
        assert!(!r.ts_output.is_empty(), "恢复草稿后应当已经换算过");
        assert!(!r.date_output.is_empty());
    }

    #[test]
    fn bogus_timezone_in_draft_falls_back_to_default() {
        // app.ron 是用户可写的。
        let mut t = TimestampTool::default();
        let before = t.tz;
        t.load_draft(r#"{"tz":"Not/AZone","ts_input":"","date_input":""}"#);
        assert_eq!(t.tz, before);
    }
}
