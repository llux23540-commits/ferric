//! JSON 工具的粘贴记录：**一条记录 = 一个文件**。
//!
//! # 为什么是文件而不是数据库
//!
//! - 记录本身就是一份**文档**（粘进来的 JSON 正文），文件系统天生存这个；
//! - 名字就是文件名 —— 用户在「设置 → 数据 → 打开数据文件夹」里直接看到、
//!   拷得走、也能用别的工具改；数据库还得先做一层导出才能给人；
//! - 增 / 删 / 改名 / 读全是一次文件系统调用，用不到事务、索引与联表；
//! - 不引 `rusqlite`：它自带一份 C 的 SQLite 要编译，六个平台的构建时间与
//!   安装包体积都得跟着涨，而换来的能力这里一条都用不上。
//!
//! 目录：`<数据目录>/json-history/<名字>.json`。
//!
//! 元数据全部**从文件系统现取**（修改时间、字节数），所以没有第二份需要
//! 同步的索引 —— 用户手动删掉一个文件，界面下次打开就是对的。

use std::path::{Path, PathBuf};

/// 记录目录名（数据目录下）。
pub const DIR: &str = "json-history";
/// 单条上限。再大的粘贴不进记录：这是「随手翻回去」用的，不是备份工具。
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// 预览只读文件开头这么多字节，不整份读进来（记录可能有几 MB）。
const PREVIEW_BYTES: usize = 4096;
const PREVIEW_CHARS: usize = 110;
/// 文件名长度上限（字符）。Windows 的 260 字符路径上限扣掉目录还剩不少，
/// 但过长的名字在抽屉里也读不出来。
const NAME_MAX_CHARS: usize = 60;

/// 一条记录。`name` 就是文件名（不含 `.json`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// 修改时间（unix 秒）
    pub at: i64,
    pub bytes: u64,
    /// 开头一小段（折叠过空白），抽屉里用来认内容
    pub preview: String,
}

pub struct JsonHistory {
    dir: Option<PathBuf>,
    /// 新的在前
    entries: Vec<Entry>,
    query: String,
    /// 最多留多少条，超出删最旧的。
    max_entries: usize,
    loaded: bool,
}

impl Default for JsonHistory {
    fn default() -> Self {
        Self {
            dir: crate::launch::data_dir().map(|d| d.join(DIR)),
            entries: Vec::new(),
            query: String::new(),
            max_entries: 200,
            loaded: false,
        }
    }
}

impl JsonHistory {
    /// 指定目录（测试用；正常路径走 [`Default`]）。
    pub fn with_dir(dir: PathBuf, max_entries: usize) -> Self {
        Self {
            dir: Some(dir),
            entries: Vec::new(),
            query: String::new(),
            max_entries,
            loaded: false,
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// 扫一遍目录。重复调用会重新扫 —— 用户可能在文件管理器里动过。
    pub fn load(&mut self) {
        self.loaded = true;
        self.entries.clear();
        let Some(dir) = self.dir.clone() else { return };
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return; // 目录还不存在 = 一条记录都没有，不是错误
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(meta) = ent.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            self.entries.push(Entry {
                name: name.to_owned(),
                at: mtime_secs(&meta),
                bytes: meta.len(),
                preview: preview_of(&path),
            });
        }
        // 新的在前：抽屉里最想看到的就是刚才那条
        self.entries
            .sort_by(|a, b| b.at.cmp(&a.at).then(a.name.cmp(&b.name)));
        self.prune();
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// 记一条。返回落盘用的名字；没记（空、过大、与最新一条重复）返回 `None`。
    pub fn record(&mut self, text: &str) -> Option<String> {
        let dir = self.dir.clone()?;
        if text.trim().is_empty() || text.len() as u64 > MAX_BYTES {
            return None;
        }
        if !self.loaded {
            self.load();
        }
        // 与最新一条一模一样就不再记：Ctrl+A、Ctrl+V 连按两下是很自然的操作，
        // 不该在记录里留两条同样的东西。
        if let Some(top) = self.entries.first() {
            if top.bytes == text.len() as u64
                && self.text_by_name(&top.name).as_deref() == Some(text)
            {
                return None;
            }
        }
        std::fs::create_dir_all(&dir).ok()?;
        let name = self.unique_name(&auto_name());
        std::fs::write(dir.join(format!("{name}.json")), text).ok()?;
        self.entries.insert(
            0,
            Entry {
                at: now_secs(),
                bytes: text.len() as u64,
                preview: preview_of_str(text),
                name: name.clone(),
            },
        );
        self.prune();
        Some(name)
    }

    /// 改名。`raw` 是用户输入，非法字符会被换掉；清理后为空则报错。
    pub fn rename(&mut self, index: usize, raw: &str) -> Result<String, String> {
        let dir = self.dir.clone().ok_or("没有可用的数据目录")?;
        let old = self
            .entries
            .get(index)
            .ok_or("这条记录已经不在了")?
            .name
            .clone();
        let want = sanitize(raw);
        if want.is_empty() {
            return Err("名字里没有可用的字符".to_owned());
        }
        if want == old {
            return Ok(old);
        }
        if self.entries.iter().any(|e| e.name == want) {
            return Err(format!("已经有一条叫「{want}」的记录了"));
        }
        std::fs::rename(
            dir.join(format!("{old}.json")),
            dir.join(format!("{want}.json")),
        )
        .map_err(|e| format!("改名失败：{e}"))?;
        self.entries[index].name = want.clone();
        Ok(want)
    }

    pub fn remove(&mut self, index: usize) -> Option<String> {
        let dir = self.dir.clone()?;
        let e = self.entries.get(index)?.clone();
        let _ = std::fs::remove_file(dir.join(format!("{}.json", e.name)));
        self.entries.remove(index);
        Some(e.name)
    }

    pub fn clear(&mut self) -> usize {
        let n = self.entries.len();
        if let Some(dir) = self.dir.clone() {
            for e in std::mem::take(&mut self.entries) {
                let _ = std::fs::remove_file(dir.join(format!("{}.json", e.name)));
            }
        }
        n
    }

    /// 读回正文（恢复到编辑区用）。
    pub fn text_of(&self, index: usize) -> Option<String> {
        let e = self.entries.get(index)?;
        self.text_by_name(&e.name)
    }

    fn text_by_name(&self, name: &str) -> Option<String> {
        let dir = self.dir.as_ref()?;
        std::fs::read_to_string(dir.join(format!("{name}.json"))).ok()
    }

    /// 按名字搜（忽略大小写的子串）。
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_owned();
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// 当前搜索词下要显示的条目。
    pub fn visible(&self) -> Vec<&Entry> {
        let q = self.query.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q))
            .collect()
    }

    /// `visible()` 里的第 n 条在 `entries` 里的下标 —— 界面只知道可见行号。
    pub fn index_of_visible(&self, n: usize) -> Option<usize> {
        let q = self.query.trim().to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| q.is_empty() || e.name.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .nth(n)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn unique_name(&self, base: &str) -> String {
        if !self.name_taken(base) {
            return base.to_owned();
        }
        // 同一秒内粘两次：加序号，而不是覆盖掉前一条
        for n in 2..1000 {
            let candidate = format!("{base}-{n}");
            if !self.name_taken(&candidate) {
                return candidate;
            }
        }
        format!("{base}-{}", now_secs())
    }

    fn name_taken(&self, name: &str) -> bool {
        if self.entries.iter().any(|e| e.name == name) {
            return true;
        }
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{name}.json")).exists())
            .unwrap_or(false)
    }

    /// 超出上限就删最旧的（`entries` 已按新→旧排好）。
    fn prune(&mut self) {
        while self.entries.len() > self.max_entries {
            let last = self.entries.len() - 1;
            self.remove(last);
        }
    }
}

/// 自动名字：`json-20260906-214930`，全是 ASCII 字母数字与短横 ——
/// 用户没起名时它同时也是文件名，必须在任何文件系统上都合法。
fn auto_name() -> String {
    format!("json-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
}

/// 把用户输入洗成合法文件名。
///
/// 非法字符换成 `-` 而不是删掉：`a/b` 变 `a-b` 还看得出原意，直接删成 `ab`
/// 会把两段黏在一起。Windows 的保留设备名（CON / NUL / COM1…）加前缀避开 ——
/// 那些名字在 Windows 上根本创建不出文件。
fn sanitize(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        let bad = matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            || ch.is_control()
            || ch == '\u{7f}';
        if bad {
            if !last_dash && !out.is_empty() {
                out.push('-');
                last_dash = true;
            }
            continue;
        }
        out.push(ch);
        last_dash = false;
        if out.chars().count() >= NAME_MAX_CHARS {
            break;
        }
    }
    // 结尾的点与空格：Windows 会静默吞掉，留着会让「写进去的名字」与
    // 「读回来的名字」不一致
    let trimmed = out.trim_matches(|c: char| c == '.' || c == ' ' || c == '-');
    let cleaned = trimmed.to_owned();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.iter().any(|r| cleaned.eq_ignore_ascii_case(r)) {
        return format!("_{cleaned}");
    }
    cleaned
}

fn preview_of(path: &Path) -> String {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buf = vec![0u8; PREVIEW_BYTES];
    let n = f.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    // 可能正好切在多字节字符中间：截到最后一个完整字符
    let s = match std::str::from_utf8(&buf) {
        Ok(s) => s.to_owned(),
        Err(e) => String::from_utf8_lossy(&buf[..e.valid_up_to()]).into_owned(),
    };
    preview_of_str(&s)
}

/// 折叠所有空白为单空格再截断：JSON 一格式化就全是缩进，原样截前 110 字
/// 只能看到一堆空格。
fn preview_of_str(text: &str) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= PREVIEW_CHARS {
            break;
        }
    }
    if out.chars().count() > PREVIEW_CHARS {
        out = out.chars().take(PREVIEW_CHARS).collect::<String>() + "…";
    }
    out
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 记录时间 → 抽屉里那一行短标签。
pub fn when_label(at: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(t) = Local.timestamp_opt(at, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if t.date_naive() == now.date_naive() {
        format!("今天 {}", t.format("%H:%M"))
    } else if t.year() == now.year() {
        t.format("%m-%d %H:%M").to_string()
    } else {
        t.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "ferric-hist-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn a_record_lands_on_disk_under_its_own_name() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir.clone(), 10);
        let name = h.record("{\"a\":1}").expect("应当记下来");
        // 自动名字必须是普通字母数字 —— 它同时就是文件名
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "{name}"
        );
        assert!(dir.join(format!("{name}.json")).is_file(), "文件没写出来");
        assert_eq!(h.text_of(0).as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn records_survive_a_restart() {
        // 自动保存的全部意义：不用点保存，下次打开还在。
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir.clone(), 10);
        h.record("{\"keep\":true}").unwrap();

        let mut again = JsonHistory::with_dir(dir, 10);
        again.load();
        assert_eq!(again.len(), 1);
        assert_eq!(again.text_of(0).as_deref(), Some("{\"keep\":true}"));
        assert!(again.visible()[0].preview.contains("keep"));
    }

    #[test]
    fn renaming_moves_the_file_and_keeps_the_body() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir.clone(), 10);
        let auto = h.record("{\"x\":1}").unwrap();
        let named = h.rename(0, "订单接口 返回/示例").expect("改名应当成功");
        assert_eq!(named, "订单接口 返回-示例", "非法字符要换成短横");
        assert!(!dir.join(format!("{auto}.json")).exists(), "旧文件还在");
        assert!(dir.join(format!("{named}.json")).is_file());
        assert_eq!(h.text_of(0).as_deref(), Some("{\"x\":1}"));
    }

    #[test]
    fn rename_rejects_empty_and_duplicate_names() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir, 10);
        h.record("{\"a\":1}").unwrap();
        h.record("{\"b\":2}").unwrap();
        h.rename(0, "甲").unwrap();
        assert!(h.rename(1, "///").is_err(), "全是非法字符时必须报错");
        let err = h.rename(1, "甲").expect_err("重名必须拒绝");
        assert!(err.contains("甲"), "{err}");
    }

    #[test]
    fn search_matches_the_name() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir, 10);
        h.record("{\"a\":1}").unwrap();
        h.rename(0, "Orders-API").unwrap();
        h.record("{\"b\":2}").unwrap();
        h.rename(0, "用户列表").unwrap();

        h.set_query("orders");
        let hit = h.visible();
        assert_eq!(hit.len(), 1, "按名字搜（忽略大小写）");
        assert_eq!(hit[0].name, "Orders-API");
        // 可见行号要能映射回真实下标，否则点「恢复」会恢复错的那条
        assert_eq!(h.index_of_visible(0), Some(1));
        assert_eq!(h.text_of(1).as_deref(), Some("{\"a\":1}"));

        h.set_query("列表");
        assert_eq!(h.visible()[0].name, "用户列表");
    }

    #[test]
    fn the_same_paste_twice_records_once() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir, 10);
        assert!(h.record("{\"a\":1}").is_some());
        assert!(h.record("{\"a\":1}").is_none(), "重复内容不该再记一条");
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn empty_and_oversized_pastes_are_not_recorded() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir, 10);
        assert!(h.record("   \n ").is_none());
        let huge = "x".repeat(MAX_BYTES as usize + 1);
        assert!(h.record(&huge).is_none(), "超过上限的粘贴不进记录");
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn the_oldest_records_are_pruned() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir.clone(), 3);
        for i in 0..5 {
            h.record(&format!("{{\"n\":{i}}}")).unwrap();
        }
        assert_eq!(h.len(), 3, "只留最新 3 条");
        // 删掉的那两条文件也要跟着走，不能只从内存里消失
        let on_disk = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(on_disk, 3);
        assert_eq!(h.text_of(0).as_deref(), Some("{\"n\":4}"));
    }

    #[test]
    fn deleting_and_clearing_remove_the_files() {
        let dir = temp();
        let mut h = JsonHistory::with_dir(dir.clone(), 10);
        h.record("{\"a\":1}").unwrap();
        h.record("{\"b\":2}").unwrap();
        h.remove(0).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(h.clear(), 1);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn windows_reserved_names_are_escaped() {
        // `CON.json` 在 Windows 上根本创建不出来
        assert_eq!(sanitize("con"), "_con");
        assert_eq!(sanitize("COM1"), "_COM1");
        assert_eq!(sanitize("console"), "console");
    }

    #[test]
    fn preview_folds_whitespace() {
        // 格式化过的 JSON 原样截断只能看到一堆缩进空格
        let p = preview_of_str("{\n    \"name\":   \"ferric\"\n}");
        assert_eq!(p, "{ \"name\": \"ferric\" }");
    }
}
