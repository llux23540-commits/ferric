//! 演示数据（mock）：不联网就能把插件市场与自动更新的**整条流程**跑一遍。
//!
//! 为什么需要它：没烘入 `FERRIC_SERVER_URL` 的构建里，市场与更新这两块界面只有一行
//! 「本构建未配置更新服务器」——界面改了没法看、流程改了没法验、演示时更没得展示。
//!
//! # 它模拟什么、不模拟什么
//!
//! 模拟：列表内容、版本号、大小、下载进度（**逐块推进 + 真实耗时**，所以进度条、
//! 「下载中 42%」、按钮禁用这些状态都是真的在跑）、装完之后的状态变化。
//!
//! 不模拟：加密信道、离线签名、真实字节。因此演示分支**从不**调用
//! `plugin_host::install`（那条路只接受验签通过的字节），也**从不**拉起安装程序。
//! 演示装上的插件记在自己的 `mock-plugins.json` 里，与真实插件目录互不干扰 ——
//! 真假两套状态混在一起，才是最容易骗到自己的做法。
//!
//! # 数据是固定的
//!
//! 版本号、sha256、大小都写死，不随机 —— 演示要可复现，截图和录屏才对得上；
//! 「上次看到的是 v1.2.0」这种记忆也才不会被随机数打乱。

use crate::market::MarketItem;
use crate::updater::ReleaseInfo;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// 模拟下载的总时长（毫秒）。够长到能看清进度条走，又不至于让人等得烦。
const DOWNLOAD_MS: u64 = 2_200;
/// 分多少次推进度。与 UI 的 120ms 轮询节奏相称。
const STEPS: u64 = 22;

/// 演示用的插件目录（每条：slug, 名称, 说明, 最新版本, 大小, 下载量）。
const PLUGINS: &[(&str, &str, &str, &str, i64, i64)] = &[
    (
        "url-codec",
        "URL 编解码",
        "URL 百分号编码 / 解码（RFC 3986），可选空格编成 +",
        "0.2.0",
        41_280,
        1_284,
    ),
    (
        "jwt-decode",
        "JWT 解码",
        "拆开 JWT 的三段，展示 header / payload 与过期时间（不校验签名）",
        "1.1.0",
        63_904,
        3_517,
    ),
    (
        "cron-explain",
        "Cron 表达式",
        "把 cron 表达式翻译成人话，并列出接下来几次触发时间",
        "0.4.1",
        58_112,
        902,
    ),
    (
        "hash-text",
        "文本摘要",
        "MD5 / SHA-1 / SHA-256 / CRC32，支持大小写与分隔符",
        "1.0.3",
        37_640,
        2_046,
    ),
    (
        "color-convert",
        "颜色转换",
        "HEX / RGB / HSL / HSV 互转，附对比度检查",
        "0.3.0",
        45_016,
        671,
    ),
];

/// 演示装好的插件（slug → 版本）。存盘，重启后状态还在。
type Installed = BTreeMap<String, String>;

/// 演示存档不存在时的初始状态：预装一个**旧版**插件。
///
/// 这样第一次打开市场就能同时看到三种状态 —— 未安装 / 已装最新 / 已装可更新，
/// 「更新」那条路不必先想办法把版本弄旧才能演示。
const SEED: &[(&str, &str)] = &[("url-codec", "0.1.0")];

fn seeded() -> Installed {
    SEED.iter()
        .map(|(s, v)| ((*s).to_owned(), (*v).to_owned()))
        .collect()
}

fn store_path() -> Option<PathBuf> {
    eframe::storage_dir(crate::launch::APP_ID).map(|d| d.join("mock-plugins.json"))
}

fn load_installed() -> Installed {
    let Some(p) = store_path() else {
        return seeded();
    };
    // 没有存档 / 存档坏了 → 回到初始演示状态，而不是「一个都没装」
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(seeded)
}

fn save_installed(v: &Installed) {
    let Some(p) = store_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(p, text);
    }
}

/// 造一个看起来像模像样、且**可复现**的 sha256（内容是假的，只为界面展示）。
fn fake_sha256(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(seed.as_bytes()))
}

/// 演示：拉插件列表。`query` 按名称 / slug / 简介做子串过滤。
pub fn browse(query: &str) -> Vec<MarketItem> {
    let q = query.trim().to_lowercase();
    let installed = load_installed();
    PLUGINS
        .iter()
        .filter(|(slug, name, desc, ..)| {
            q.is_empty()
                || slug.to_lowercase().contains(&q)
                || name.to_lowercase().contains(&q)
                || desc.to_lowercase().contains(&q)
        })
        .map(|(slug, name, desc, version, size, downloads)| {
            let local = installed.get(*slug).cloned();
            MarketItem {
                has_update: local.as_deref().is_some_and(|v| v != *version),
                installed: local,
                slug: (*slug).to_owned(),
                name: (*name).to_owned(),
                desc: (*desc).to_owned(),
                version: (*version).to_owned(),
                api_version: ferric_core::plugin::API_VERSION as i64,
                size: *size,
                sha256: fake_sha256(&format!("{slug}@{version}")),
                // 演示数据不参与验签（那条路只走真实源），给个显眼的占位串，
                // 万一哪天被误接到真实安装路径上，验签会立刻失败而不是悄悄放行。
                signature: "MOCK-NOT-A-REAL-SIGNATURE".to_owned(),
                downloads: *downloads,
            }
        })
        .collect()
}

/// 演示：哪些已装插件有新版。
pub fn check_updates() -> Vec<String> {
    let installed = load_installed();
    PLUGINS
        .iter()
        .filter(|(slug, _, _, version, ..)| installed.get(*slug).is_some_and(|v| v != version))
        .map(|(slug, ..)| (*slug).to_owned())
        .collect()
}

/// 演示：安装一个插件。走一遍带进度的「下载」，然后把版本记进演示存档。
///
/// **不写真实插件目录** —— 演示数据没有可验证的字节，写进去等于绕过验签。
/// 所以演示装上的插件不会出现在侧栏，界面上对此有说明。
pub fn install(slug: &str, version: &str, size: i64, on_progress: &mut dyn FnMut(u64, u64)) {
    simulate_download(size.max(1) as u64, on_progress);
    let mut installed = load_installed();
    installed.insert(slug.to_owned(), version.to_owned());
    save_installed(&installed);
}

/// 演示：卸载。
pub fn uninstall(slug: &str) {
    let mut installed = load_installed();
    installed.remove(slug);
    save_installed(&installed);
}

/// 演示：把状态恢复成初始的样子（删掉存档即可，读的时候会回到 [`SEED`]）。
///
/// 演示是要反复做的：装完一轮之后所有插件都成了「已装最新」，
/// 没有这个按钮就只能手动去删那个 json 才能再演一遍。
pub fn reset_demo() {
    if let Some(p) = store_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// 演示：检查应用更新。永远「有新版」—— 演示要的就是这条路径。
///
/// build 取本机 +7：既保证 `updater` 的本地重算判定为「有更新」，
/// 又不至于大到看着像脏数据。
pub fn latest_release() -> ReleaseInfo {
    let build = crate::updater::my_build() + 7;
    let version = next_version(crate::updater::my_version());
    let ext = match std::env::consts::OS {
        "windows" => ".exe",
        "macos" => ".dmg",
        _ => ".AppImage",
    };
    ReleaseInfo {
        id: 1,
        sha256: fake_sha256(&format!("ferric@{version}")),
        size: 8_642_048,
        min_supported_build: 0,
        signature: "MOCK-NOT-A-REAL-SIGNATURE".to_owned(),
        notes: "演示数据：修复了若干问题，新增插件市场后台下载。\n\
                （这是本地模拟的版本信息，不会真的安装任何东西）"
            .to_owned(),
        ext: ext.to_owned(),
        force: false,
        version,
        build,
    }
}

/// 末位 +1 的下一个版本号；形状不对时退回原串加后缀。
fn next_version(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() == 3 {
        if let Ok(patch) = parts[2].parse::<u64>() {
            return format!("{}.{}.{}", parts[0], parts[1], patch + 1);
        }
    }
    format!("{v}-next")
}

/// 演示：下载安装包。只是把进度跑完，**不产生任何文件**。
///
/// 返回的路径是个不存在的占位 —— 演示分支下 `Source::allows_install()` 为 false，
/// 界面不会拿它去执行任何东西。
pub fn download_release(info: &ReleaseInfo, on_progress: &mut dyn FnMut(u64, u64)) -> PathBuf {
    simulate_download(info.size.max(1) as u64, on_progress);
    PathBuf::from(format!("(演示){}-{}{}", info.version, info.build, info.ext))
}

/// 分块推进度 + 真实 sleep，让进度条真的在走。
fn simulate_download(total: u64, on_progress: &mut dyn FnMut(u64, u64)) {
    on_progress(0, total);
    for i in 1..=STEPS {
        std::thread::sleep(Duration::from_millis(DOWNLOAD_MS / STEPS));
        on_progress(total * i / STEPS, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 演示列表必须是**可复现**的：同样的输入两次调用完全一致。
    /// 截图、录屏、回归测试都靠这个。
    #[test]
    fn listing_is_deterministic() {
        let a = browse("");
        let b = browse("");
        assert_eq!(a.len(), PLUGINS.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.slug, y.slug);
            assert_eq!(x.sha256, y.sha256, "sha256 必须可复现，不能随机生成");
            assert_eq!(x.version, y.version);
        }
    }

    /// 搜索要能按名称 / slug / 简介过滤（中英文都得能搜到）。
    #[test]
    fn search_filters_by_name_slug_and_desc() {
        assert_eq!(browse("jwt").len(), 1);
        assert_eq!(browse("JWT").len(), 1, "搜索应不分大小写");
        assert!(browse("颜色").iter().any(|i| i.slug == "color-convert"));
        assert!(browse("cron").iter().any(|i| i.slug == "cron-explain"));
        assert!(browse("不存在的插件").is_empty());
    }

    /// 演示的版本号必须让 `updater` 的**本地重算**判定为「有更新」——
    /// 否则演示流程第一步就走不下去。
    #[test]
    fn mock_release_is_newer_than_this_build() {
        let r = latest_release();
        assert!(
            r.build > crate::updater::my_build(),
            "演示版本 build {} 不比本机 {} 新",
            r.build,
            crate::updater::my_build()
        );
        assert!(!r.ext.is_empty(), "扩展名要按平台给");
        assert_eq!(r.size, 8_642_048);
    }

    /// 版本号推进规则：末位 +1；形状不对时不 panic。
    #[test]
    fn version_bump() {
        assert_eq!(next_version("0.2.0"), "0.2.1");
        assert_eq!(next_version("1.9.99"), "1.9.100");
        assert_eq!(next_version("weird"), "weird-next");
        assert_eq!(next_version("1.2"), "1.2-next");
    }

    /// 进度回调必须**从 0 走到 total**，且单调不回退 ——
    /// 进度条跳回去比不动还难看。
    #[test]
    fn progress_is_monotonic_and_completes() {
        let mut seen: Vec<(u64, u64)> = Vec::new();
        simulate_download(1000, &mut |d, t| seen.push((d, t)));
        assert_eq!(seen.first(), Some(&(0, 1000)), "应从 0 开始");
        assert_eq!(seen.last(), Some(&(1000, 1000)), "必须走到 100%");
        assert!(seen.len() > 5, "太少的进度点看不出在动：{}", seen.len());
        assert!(
            seen.windows(2).all(|w| w[1].0 >= w[0].0),
            "进度不能回退：{seen:?}"
        );
    }

    /// 演示数据的签名字段必须是个**显眼的假值**：万一哪天被误接到真实安装路径上，
    /// 验签会立刻失败，而不是因为空串走进别的分支被悄悄放行。
    #[test]
    fn mock_signatures_are_obviously_fake_and_not_empty() {
        for it in browse("") {
            assert!(
                it.signature.contains("MOCK"),
                "{}: {}",
                it.slug,
                it.signature
            );
            assert!(!it.signature.trim().is_empty());
        }
        assert!(latest_release().signature.contains("MOCK"));
    }

    /// 初始演示状态里必须有一个**旧版**插件：一进市场就能看到「可更新」那条路，
    /// 不必先想办法把版本弄旧。
    #[test]
    fn seed_state_shows_all_three_states() {
        let s = seeded();
        assert!(!s.is_empty(), "初始演示状态是空的，看不到「已装 / 可更新」");
        for (slug, ver) in SEED {
            let latest = PLUGINS
                .iter()
                .find(|(p, ..)| p == slug)
                .unwrap_or_else(|| panic!("预装的 {slug} 不在演示目录里"));
            assert_ne!(
                latest.3, *ver,
                "预装的 {slug} 就是最新版，演示不出「可更新」"
            );
        }
        // 目录里也得有没被预装的（「安装」那条路）
        assert!(
            PLUGINS.iter().any(|(slug, ..)| !s.contains_key(*slug)),
            "所有插件都预装了，演示不出「安装」"
        );
    }

    /// 演示的安装状态存在自己的档里，且能被读回来（这条同时覆盖存/取/卸载）。
    ///
    /// 注意：它会写到真实的配置目录，因此只在配置目录可定位时跑。
    #[test]
    fn install_records_state_and_reports_updates() {
        if store_path().is_none() {
            eprintln!("跳过：定位不到配置目录");
            return;
        }
        let before = load_installed();
        uninstall("hash-text");

        // 装一个旧版 → 应当报「有更新」
        let mut installed = load_installed();
        installed.insert("hash-text".into(), "0.0.1".into());
        save_installed(&installed);
        assert!(check_updates().contains(&"hash-text".to_owned()));
        assert!(browse("hash")[0].has_update, "旧版应当标为可更新");

        // 装成最新版 → 不再有更新
        install("hash-text", "1.0.3", 100, &mut |_, _| {});
        assert!(!check_updates().contains(&"hash-text".to_owned()));
        let it = &browse("hash")[0];
        assert_eq!(it.installed.as_deref(), Some("1.0.3"));
        assert!(!it.has_update);

        uninstall("hash-text");
        assert!(
            browse("hash")[0].installed.is_none(),
            "卸载后不该还显示已装"
        );
        save_installed(&before); // 还原，别把开发者的演示状态弄乱
    }
}
