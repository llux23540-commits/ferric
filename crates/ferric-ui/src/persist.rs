//! 界面状态持久化。
//!
//! egui 时代这一层是 `eframe::Storage`（eframe 自己在状态目录写 `app.ron`）。
//! 迁 Slint 后 eframe 没了，这里自己管：**同一个目录、同一个文件名、同一份
//! JSON 结构**，所以老用户升级后设置与草稿都还在。
//!
//! 目录与 `launch.json` / `startup.log` 同处一地（见 [`crate::launch::data_dir`]）：
//! - Windows: `%APPDATA%\ferric\data\`
//! - macOS:   `~/Library/Application Support/ferric/`
//! - Linux:   `~/.local/share/ferric/`
//!
//! 失败一律静默降级为默认值 —— 坏的状态文件不该成为打不开应用的理由。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 主题模式：默认跟随系统深浅色，也可手动锁定。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    /// Slint 的 Segmented 用索引表达，这里做双向映射（顺序 = 系统 / 亮 / 暗）。
    pub const ALL: [ThemeMode; 3] = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

    pub fn index(self) -> i32 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as i32
    }

    pub fn from_index(i: i32) -> Self {
        Self::ALL
            .get(i.max(0) as usize)
            .copied()
            .unwrap_or(ThemeMode::System)
    }
}

fn default_true() -> bool {
    true
}

fn default_ui_scale() -> f32 {
    1.0
}

fn default_rail_width() -> f32 {
    264.0
}

fn default_active_id() -> String {
    "json".to_owned()
}

/// 落盘的界面状态。
///
/// ⚠️ 字段名与 egui 版**逐字一致**（`#[serde(default)]` 保证缺字段也读得出来）。
/// 改名 = 老用户那一项设置回默认值。新增字段必须带 `default`。
#[derive(Serialize, Deserialize)]
pub struct Persist {
    /// 最近一次生效的深浅色（启动首帧兜底，避免闪白/闪黑）
    pub dark: bool,
    #[serde(default)]
    pub theme_mode: Option<ThemeMode>,
    #[serde(default = "default_rail_width")]
    pub rail_width: f32,
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default = "default_active_id")]
    pub active_id: String,
    #[serde(default)]
    pub drafts: HashMap<String, String>,
    #[serde(default)]
    pub lang: crate::tool::Lang,
    /// 自定义更新服务器（地址 + 公钥**作为整体**存取，禁止只改其一）。
    #[serde(default)]
    pub server: Option<crate::net::ServerProfile>,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// 自动检查更新并在后台下载。默认开 —— 更新只有及时装上才有意义。
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// 旧字段：是否使用演示数据。已被 `source_pref` 取代，保留做双向兼容。
    #[serde(default)]
    pub mock_source: Option<bool>,
    #[serde(default)]
    pub source_pref: Option<crate::source::SourcePref>,
    #[serde(default)]
    pub github_repo: Option<String>,
    /// 上次成功检查更新的 Unix 时间戳（秒）。跨启动节流用。
    #[serde(default)]
    pub last_update_check: Option<i64>,
}

impl Default for Persist {
    fn default() -> Self {
        Self {
            dark: false,
            theme_mode: Some(ThemeMode::System),
            rail_width: default_rail_width(),
            favorites: Vec::new(),
            active_id: default_active_id(),
            drafts: HashMap::new(),
            lang: crate::tool::Lang::default(),
            server: None,
            ui_scale: 1.0,
            auto_update: true,
            mock_source: None,
            source_pref: None,
            github_repo: None,
            last_update_check: None,
        }
    }
}

/// 状态文件名。与 eframe 的默认一致，因此 egui 版写下的那份直接能读。
const FILE: &str = "app.ron";

fn path() -> Option<PathBuf> {
    crate::launch::data_dir().map(|d| d.join(FILE))
}

/// 读状态。读不到 / 解析不了一律用默认值。
pub fn load() -> Persist {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

/// 写状态。先写临时文件再改名，避免写到一半的文件被下次启动读到。
pub fn save(p: &Persist) {
    if let Some(path) = path() {
        save_to(&path, p);
    }
}

// 下面两个按路径操作的版本是为了**可测**：真实路径落在用户配置目录里，
// 单测不该往那儿写东西。

fn load_from(p: &std::path::Path) -> Persist {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_to(p: &std::path::Path, persist: &Persist) {
    let Ok(json) = serde_json::to_string_pretty(persist) else {
        return;
    };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("ron.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferric-persist-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn roundtrip_preserves_settings_and_drafts() {
        let dir = tmpdir();
        let p = dir.join("rt.ron");
        let mut want = Persist {
            dark: true,
            theme_mode: Some(ThemeMode::Dark),
            rail_width: 320.0,
            favorites: vec!["uuid".into(), "json".into()],
            active_id: "uuid".into(),
            ui_scale: 1.25,
            auto_update: false,
            ..Default::default()
        };
        want.drafts.insert("uuid".into(), r#"{"count":42}"#.into());

        save_to(&p, &want);
        let got = load_from(&p);

        assert!(got.dark);
        assert_eq!(got.theme_mode, Some(ThemeMode::Dark));
        assert_eq!(got.rail_width, 320.0);
        assert_eq!(got.favorites, vec!["uuid", "json"]);
        assert_eq!(got.active_id, "uuid");
        assert_eq!(
            got.drafts.get("uuid").map(|s| s.as_str()),
            Some(r#"{"count":42}"#)
        );
        assert_eq!(got.ui_scale, 1.25);
        assert!(!got.auto_update);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn legacy_file_without_new_fields_still_loads() {
        // egui 早期版本写下的最小状态：只有 dark / rail_width / active_id。
        // 这条断言就是「升级不丢设置」的守门人。
        let dir = tmpdir();
        let p = dir.join("legacy.ron");
        std::fs::write(
            &p,
            r#"{"dark":true,"rail_width":300.0,"active_id":"diff","favorites":["sql"]}"#,
        )
        .unwrap();

        let got = load_from(&p);
        assert!(got.dark);
        assert_eq!(got.rail_width, 300.0);
        assert_eq!(got.active_id, "diff");
        assert_eq!(got.favorites, vec!["sql"]);
        // 缺失字段走默认，而不是整份回默认
        assert!(got.auto_update, "auto_update 缺失应默认开");
        assert_eq!(got.ui_scale, 1.0);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_instead_of_panicking() {
        let dir = tmpdir();
        let p = dir.join("corrupt.ron");
        std::fs::write(&p, "{ this is not json ").unwrap();
        let got = load_from(&p);
        assert_eq!(got.active_id, "json", "坏文件必须静默回默认");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let got = load_from(std::path::Path::new("/definitely/not/here/app.ron"));
        assert_eq!(got.active_id, "json");
        assert_eq!(got.theme_mode, Some(ThemeMode::System));
    }

    #[test]
    fn theme_mode_index_mapping_is_bidirectional() {
        for (i, m) in ThemeMode::ALL.iter().enumerate() {
            assert_eq!(ThemeMode::from_index(i as i32), *m);
            assert_eq!(m.index(), i as i32);
        }
        // 越界索引回落到跟随系统，而不是 panic
        assert_eq!(ThemeMode::from_index(99), ThemeMode::System);
        assert_eq!(ThemeMode::from_index(-1), ThemeMode::System);
    }
}
