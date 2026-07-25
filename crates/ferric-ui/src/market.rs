//! 插件市场：浏览、检查更新、下载安装。
//!
//! 与应用自动更新（`updater`）共用同一条信任链：加密信道（`net`）+ 一次性下载票据 +
//! **sha256 取自加密信道**（绝不用明文的 `X-Sha256` 响应头）。
//!
//! # 与应用安装包的一个区别：插件不做离线签名
//!
//! 应用安装包会被原生执行，所以必须验离线签名。插件是 wasm，跑在 wasmtime 沙箱里
//! —— 无文件、无网络、无系统调用，还有燃料与内存上限（见 `plugin_host` 模块头）——
//! 爆炸半径小得多，本轮先只做 sha256 校验。
//! ⚠️ 但这不是「插件绝对安全」：一个恶意插件仍能返回错误的计算结果（比如伪造一个
//! 「加密工具」给出可预测的输出）。给插件也加签名是明确的后续项。
//!
//! # 版本号从哪来
//!
//! 从每个 `.wasm` 内部的 manifest 读（`plugin_host::installed`），不靠文件名、
//! 不靠旁挂索引 —— 版本与代码物理绑定，不可能出现两者不一致。老插件没有该字段时
//! 版本为空串，服务端约定此时一律按「有更新」处理。

use crate::net::{self, ServerProfile};
use crate::plugin_host;
use sha2::{Digest, Sha256};

/// 一次能问的插件数上限，与服务端 `CHECK_UPDATES_MAX` 对齐。
const CHECK_MAX: usize = 200;
/// 单个 wasm 大小上限，与服务端 `PLUGIN_MAX_BYTES` 对齐。
const PLUGIN_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// 市场里的一条插件。
#[derive(Debug, Clone)]
pub struct MarketItem {
    pub slug: String,
    pub name: String,
    pub desc: String,
    pub version: String,
    pub api_version: i64,
    pub size: i64,
    pub sha256: String,
    pub downloads: i64,
    /// 本地已装的版本；None 表示没装
    pub installed: Option<String>,
    /// 本地已装但服务端有更新
    pub has_update: bool,
}

/// 拉市场列表，并与本地已装插件对账。
pub fn browse(profile: &ServerProfile, query: &str) -> Result<Vec<MarketItem>, String> {
    // 只要与本宿主 api_version 完全相同的插件 —— 宿主侧是严格相等校验，
    // 装了别的版本也加载不了，不如根本不展示。
    let api_version = ferric_core::plugin::API_VERSION;
    let path = format!(
        "/plugins?query={}&api_version={api_version}&page=1&size=100",
        urlencode(query)
    );
    let v = net::call(profile, "GET", &path, None).map_err(|e| e.to_string())?;
    let list = v
        .get("list")
        .and_then(|x| x.as_array())
        .ok_or("响应缺少 list")?;

    let installed = plugin_host::installed();
    let mut out = Vec::new();
    for it in list {
        let s = |k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
        let n = |k: &str| it.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        let slug = s("slug");
        if slug.is_empty() {
            continue;
        }
        let local = installed
            .iter()
            .find(|i| i.slug == slug)
            .map(|i| i.version.clone());
        out.push(MarketItem {
            has_update: false, // 由 check_updates 填，本地不猜版本大小
            installed: local,
            version: s("version"),
            name: s("name"),
            desc: s("desc"),
            api_version: n("api_version"),
            size: n("size"),
            sha256: s("sha256"),
            downloads: n("downloads"),
            slug,
        });
    }
    Ok(out)
}

/// 问服务端：本地这些插件有没有新版。
///
/// 版本比较交给服务端做 —— 它有 semver 排序的权威数据（`sort_key`），
/// 客户端自己比字符串迟早会在 `0.10.0` vs `0.9.0` 上翻车。
pub fn check_updates(profile: &ServerProfile) -> Result<Vec<String>, String> {
    let installed = plugin_host::installed();
    if installed.is_empty() {
        return Ok(Vec::new());
    }
    let plugins: Vec<serde_json::Value> = installed
        .iter()
        .take(CHECK_MAX)
        .map(|i| serde_json::json!({ "slug": i.slug, "version": i.version }))
        .collect();
    let body = serde_json::json!({
        "api_version": ferric_core::plugin::API_VERSION,
        "plugins": plugins
    });
    let v = net::call(profile, "POST", "/plugins/check-updates", Some(&body))
        .map_err(|e| e.to_string())?;
    let list = v
        .get("list")
        .and_then(|x| x.as_array())
        .ok_or("响应缺少 list")?;

    Ok(list
        .iter()
        .filter(|r| r.get("has_update").and_then(|x| x.as_bool()) == Some(true))
        .filter_map(|r| r.get("slug").and_then(|x| x.as_str()).map(str::to_owned))
        .collect())
}

/// 下载并安装一个插件版本。
///
/// `expect_sha256` 必须来自**加密信道**（列表或 check-updates 的响应），
/// 绝不能用下载响应里的明文 `X-Sha256` 头 —— 那个头中间人可以连同内容一起改。
pub fn install(
    profile: &ServerProfile,
    slug: &str,
    version: Option<&str>,
    expect_sha256: &str,
    expect_size: i64,
) -> Result<(), String> {
    if expect_sha256.len() != 64 {
        return Err("服务端未提供有效的 sha256，已中止安装".into());
    }
    if expect_size <= 0 || expect_size as u64 > PLUGIN_MAX_BYTES {
        return Err(format!("插件大小异常：{expect_size} 字节"));
    }

    // 换一次性票据（走加密信道）
    let mut body = serde_json::json!({ "kind": "plugin", "slug": slug });
    if let Some(v) = version {
        body["version"] = serde_json::Value::String(v.to_owned());
    }
    let t =
        net::call(profile, "POST", "/download-ticket", Some(&body)).map_err(|e| e.to_string())?;
    let url = t
        .get("url")
        .and_then(|x| x.as_str())
        .ok_or("换取下载票据失败")?;
    // 只当路径用，绝不接受指向别的 host 的绝对 URL
    if !url.starts_with('/') {
        return Err("服务端返回的下载地址非法".into());
    }
    let path = url.strip_prefix("/api/v1").unwrap_or(url);

    // wasm 只有 10MB 上限，直接收在内存里即可（不像安装包要流式落盘）
    let mut buf: Vec<u8> = Vec::with_capacity(expect_size as usize);
    net::download_to(profile, path, expect_size as u64, &mut |chunk| {
        buf.extend_from_slice(chunk);
        Ok(())
    })
    .map_err(|e| e.to_string())?;

    if buf.len() as i64 != expect_size {
        return Err(format!(
            "大小不符：期望 {expect_size} 字节，实际 {}",
            buf.len()
        ));
    }
    let got = hex::encode(Sha256::digest(&buf));
    if !got.eq_ignore_ascii_case(expect_sha256) {
        return Err("内容校验失败：sha256 与服务端在加密信道中声明的不一致".into());
    }
    // 魔数自检：必须是 wasm，别把随便什么东西写进插件目录
    if !buf.starts_with(b"\0asm") {
        return Err("下载内容不是有效的 WASM 模块，已拒绝".into());
    }

    plugin_host::install(slug, &buf)?;
    Ok(())
}

/// 极简 percent-encode（只保留 RFC 3986 unreserved 字符），给 query 用。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_query_metachars() {
        assert_eq!(urlencode("json"), "json");
        assert_eq!(urlencode("a b"), "a%20b");
        // 这几个如果不转义就能篡改 query 结构
        assert_eq!(urlencode("a&b=c"), "a%26b%3Dc");
        assert_eq!(urlencode("中"), "%E4%B8%AD");
    }

    /// 校验和 / 大小的守卫必须在联网之前就把明显非法的输入挡掉。
    #[test]
    fn install_rejects_bad_metadata_before_network() {
        let p = ServerProfile {
            base_url: "http://127.0.0.1:1".into(), // 故意指向必然连不上的端口
            pubkey: format!("04{}", "ab".repeat(64)),
        };
        // sha256 长度不对 —— 必须在发起任何请求之前就失败
        let e = install(&p, "demo", None, "abc", 10).unwrap_err();
        assert!(e.contains("sha256"), "{e}");
        // 大小非法
        let e = install(&p, "demo", None, &"a".repeat(64), 0).unwrap_err();
        assert!(e.contains("大小"), "{e}");
        let e = install(&p, "demo", None, &"a".repeat(64), 99_000_000).unwrap_err();
        assert!(e.contains("大小"), "{e}");
    }
}

/// 对着**真实运行的服务端**跑一遍插件市场全链路。
/// 默认不跑；需 `FERRIC_E2E=1` 且烘入了服务器身份。
#[cfg(test)]
mod e2e {
    use super::*;

    #[test]
    fn e2e_browse_install_and_verify() {
        if std::env::var("FERRIC_E2E").as_deref() != Ok("1") {
            eprintln!("跳过：未设置 FERRIC_E2E=1");
            return;
        }
        let profile = ServerProfile::builtin().expect("需要烘入服务器身份");

        // 从干净状态开始，免得上一轮的残留干扰断言
        let _ = plugin_host::uninstall("url-codec");

        let items = browse(&profile, "").expect("拉市场列表失败");
        eprintln!("市场共 {} 个插件", items.len());
        let it = items
            .iter()
            .find(|i| i.slug == "url-codec")
            .expect("市场里应有 url-codec");
        eprintln!(
            "  {}@{} sha={}… {}B 已装={:?}",
            it.slug,
            it.version,
            &it.sha256[..12],
            it.size,
            it.installed
        );
        assert!(it.installed.is_none(), "起始状态应为未安装");
        assert_eq!(it.api_version, ferric_core::plugin::API_VERSION as i64);

        // 安装：换票 → 下载 → sha256 校验 → wasm 魔数 → 落盘
        install(&profile, &it.slug, Some(&it.version), &it.sha256, it.size).expect("安装失败");

        // 必须固定写成 <slug>.wasm，否则 load_all 的字典序陷阱会让旧版赢
        let path = plugin_host::plugins_dir().unwrap().join("url-codec.wasm");
        assert!(path.is_file(), "必须安装为 {}", path.display());
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            hex::encode(Sha256::digest(&bytes)),
            it.sha256,
            "落盘内容必须与声明一致"
        );

        // 宿主要能真正加载它，并读出 manifest 里的版本号
        let inst = plugin_host::installed();
        let got = inst
            .iter()
            .find(|i| i.slug == "url-codec")
            .expect("宿主应能枚举到刚装的插件");
        eprintln!("宿主读到：{} v{} ({})", got.slug, got.version, got.name);
        assert_eq!(
            got.version, it.version,
            "manifest 里的版本必须与上架版本一致"
        );

        // 装好之后再问一次更新：不应再有更新
        let updatable = check_updates(&profile).expect("检查更新失败");
        assert!(
            !updatable.contains(&"url-codec".to_owned()),
            "刚装的就是最新版，不该报有更新：{updatable:?}"
        );

        // sha256 对不上必须拒绝（把 sha 改一位）
        let mut bad = it.sha256.clone();
        let last = bad.pop().unwrap();
        bad.push(if last == 'a' { 'b' } else { 'a' });
        let e = install(&profile, &it.slug, Some(&it.version), &bad, it.size).unwrap_err();
        assert!(e.contains("校验失败"), "sha256 不符必须拒绝：{e}");

        let _ = plugin_host::uninstall("url-codec");
        eprintln!("插件市场全链路通过");
    }
}
