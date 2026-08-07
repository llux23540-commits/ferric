//! 插件市场：浏览、检查更新、下载安装。
//!
//! 与应用自动更新（`updater`）共用**同一条完整信任链**：加密信道（`net`）+ 一次性下载
//! 票据 + **sha256 取自加密信道**（绝不用明文的 `X-Sha256` 响应头）+ **离线签名**。
//!
//! # 插件为什么也要验离线签名
//!
//! 沙箱管的是「插件能碰到什么」（无文件、无网络、无系统调用，还有燃料与内存上限，
//! 见 `plugin_host` 模块头），管不了「插件算出什么」—— 一个恶意的「加密工具」插件
//! 完全可以在沙箱内规规矩矩地输出可预测的密文。
//!
//! 而 sha256 只保证「拿到的确实是服务器库里那个文件」，服务器一旦被拿下，攻击者上传的
//! 插件同样会让公钥固定 + sha256 + GCM 全部正常通过。所以插件走与安装包相同的离线签名：
//! 私钥永不上服务器，沦陷后果从「向全网推恶意插件」降级为「拒绝服务」。
//!
//! 清单里绑了 slug（见 `release::plugin_signing_payload`），因此**换个身份重放**也不行 ——
//! 拿「计算器」那份合法签名的 wasm 冒充「加密工具」下发，客户端按 slug 组装的待签字节
//! 对不上，验签必失败。
//!
//! ⚠️ 签名挡不住的是**降级重放**：把某个仍有合法签名的旧版本当成最新版下发。
//! 插件安装是用户主动发起、版本号就在界面上，加之服务端 `sort_key` 保证 latest 正确，
//! 这里不额外做本地版本单调性校验（应用更新那边有 `build` 可比，插件没有可信的本地基准）。
//!
//! # 版本号从哪来
//!
//! 从每个 `.wasm` 内部的 manifest 读（`plugin_host::installed`），不靠文件名、
//! 不靠旁挂索引 —— 版本与代码物理绑定，不可能出现两者不一致。老插件没有该字段时
//! 版本为空串，服务端约定此时一律按「有更新」处理。

use crate::net::{self, ServerProfile};
use crate::plugin_host;
use crate::release;
use crate::source::Source;
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
    /// 离线签名（DER hex）。空串 = 服务端未给这个版本背书，本地一律拒绝安装。
    pub signature: String,
    pub downloads: i64,
    /// 本地已装的版本；None 表示没装
    pub installed: Option<String>,
    /// 本地已装但服务端有更新
    pub has_update: bool,
}

// ============================ 数据源分发 ============================
//
// 下面三个 `pub fn` 只做一件事：按数据源选实现。真实分支（`*_server`）的函数体
// 与接入演示数据之前**一字未改** —— 那条路上的每一处校验都还在原地，
// 演示分支是完全独立的另一条实现（见 `mock` 模块头）。

/// 拉市场列表（真服务端或演示数据）。
pub fn browse(source: &Source, query: &str) -> Result<Vec<MarketItem>, String> {
    match source {
        Source::Server(p) => browse_server(p, query),
        Source::Mock => Ok(crate::mock::browse(query)),
    }
}

/// 问「本地这些插件有没有新版」。
pub fn check_updates(source: &Source) -> Result<Vec<String>, String> {
    match source {
        Source::Server(p) => check_updates_server(p),
        Source::Mock => Ok(crate::mock::check_updates()),
    }
}

/// 下载并安装一个插件版本，`on_progress(已下载, 总大小)` 用于驱动进度条。
pub fn install(
    source: &Source,
    it: &MarketItem,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    match source {
        Source::Server(p) => install_server(p, it, on_progress),
        Source::Mock => {
            crate::mock::install(&it.slug, &it.version, it.size, on_progress);
            Ok(())
        }
    }
}

/// 卸载。真实源删插件目录里的文件，演示源只改演示存档。
pub fn uninstall(source: &Source, slug: &str) -> Result<(), String> {
    match source {
        Source::Server(_) => plugin_host::uninstall(slug),
        Source::Mock => {
            crate::mock::uninstall(slug);
            Ok(())
        }
    }
}

/// 演示源专用：把演示状态恢复成初始的样子（真实源无操作）。
pub fn reset_demo(source: &Source) {
    if source.is_mock() {
        crate::mock::reset_demo();
    }
}

/// 装完之后，侧栏是不是立刻就能看到这个插件。
///
/// 真实安装会落到插件目录，宿主随后热加载即可生效；演示安装**不写**插件目录
/// （没有可验签的字节），所以它永远不会出现在侧栏 —— 这一点必须如实告诉用户。
pub fn takes_effect_in_sidebar(source: &Source) -> bool {
    matches!(source, Source::Server(_))
}

// ============================ 真服务端实现 ============================

/// 拉市场列表，并与本地已装插件对账。
fn browse_server(profile: &ServerProfile, query: &str) -> Result<Vec<MarketItem>, String> {
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
            signature: s("signature"),
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
fn check_updates_server(profile: &ServerProfile) -> Result<Vec<String>, String> {
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
/// 全部期望值（sha256 / size / signature / version / api_version）必须来自**加密信道**
/// （列表或 check-updates 的响应），绝不能用下载响应里的明文 `X-Sha256` 头 ——
/// 那个头中间人可以连同内容一起改。
///
/// 校验顺序：先用离线签名确认「这份元数据是发布者签发的」，再用其中的 sha256
/// 确认「拿到的字节就是被签的那份」。两步都过才落盘。
fn install_server(
    profile: &ServerProfile,
    it: &MarketItem,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let (slug, expect_sha256, expect_size) = (&it.slug, &it.sha256, it.size);
    if expect_sha256.len() != 64 {
        return Err("服务端未提供有效的 sha256，已中止安装".into());
    }
    if expect_size <= 0 || expect_size as u64 > PLUGIN_MAX_BYTES {
        return Err(format!("插件大小异常：{expect_size} 字节"));
    }

    // 验签放在联网之前：元数据没有背书就没有下载的必要。
    // 待签字节全部由**本地**按已知字段组装，服务端只提供签名本身 ——
    // 否则等于让对方自己决定「签的是什么」。
    let Some(verify_pk) = release::builtin_pubkey() else {
        return Err("本构建未烘入发布验签公钥，无法验证插件来源，已拒绝安装".into());
    };
    if it.signature.trim().is_empty() {
        return Err(
            "该插件版本未签名，已拒绝安装（沙箱限制的是能碰到什么，不是能算出什么）".into(),
        );
    }
    if it.version.trim().is_empty() {
        return Err("服务端未提供插件版本号，已中止安装".into());
    }
    let payload = release::plugin_signing_payload(
        slug,
        &it.version,
        it.api_version,
        expect_sha256,
        expect_size,
    );
    release::verify(verify_pk, &it.signature, &payload)
        .map_err(|e| format!("插件签名校验不通过：{e}"))?;

    // 换一次性票据（走加密信道）。票据绑定到**刚验过签的那个版本号**，
    // 不给服务端「签的是 A、发的是 B」的空间。
    let body = serde_json::json!({ "kind": "plugin", "slug": slug, "version": it.version });
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
    let total = expect_size as u64;
    let mut buf: Vec<u8> = Vec::with_capacity(expect_size as usize);
    on_progress(0, total);
    net::download_to(profile, path, total, &mut |chunk| {
        buf.extend_from_slice(chunk);
        // 插件只有几十 KB，不必像安装包那样按时间节流：块数本来就不多
        on_progress(buf.len() as u64, total);
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

    /// 造一条元数据齐全的市场条目（签名字段留给各用例自己填）。
    fn item(sha: &str, size: i64, signature: &str) -> MarketItem {
        MarketItem {
            slug: "demo".into(),
            name: "Demo".into(),
            desc: String::new(),
            version: "1.0.0".into(),
            api_version: 1,
            size,
            sha256: sha.into(),
            signature: signature.into(),
            downloads: 0,
            installed: None,
            has_update: false,
        }
    }

    fn unreachable_server() -> ServerProfile {
        ServerProfile {
            base_url: "http://127.0.0.1:1".into(), // 故意指向必然连不上的端口
            pubkey: format!("04{}", "ab".repeat(64)),
        }
    }

    /// 校验和 / 大小的守卫必须在联网之前就把明显非法的输入挡掉。
    #[test]
    fn install_rejects_bad_metadata_before_network() {
        let p = unreachable_server();
        let sig = "3045";
        // sha256 长度不对 —— 必须在发起任何请求之前就失败
        let e = install_server(&p, &item("abc", 10, sig), &mut |_, _| {}).unwrap_err();
        assert!(e.contains("sha256"), "{e}");
        // 大小非法
        let e = install_server(&p, &item(&"a".repeat(64), 0, sig), &mut |_, _| {}).unwrap_err();
        assert!(e.contains("大小"), "{e}");
        let e = install_server(&p, &item(&"a".repeat(64), 99_000_000, sig), &mut |_, _| {})
            .unwrap_err();
        assert!(e.contains("大小"), "{e}");
    }

    /// 未签名的版本必须在**联网之前**就被拒 —— 不下载、不落盘。
    ///
    /// 这条挡的是「服务器被拿下后直接上架一个没有背书的恶意插件」：
    /// 那种包的 sha256 与传输加密全都是对的，只有签名给不出来。
    #[test]
    fn install_refuses_unsigned_plugin() {
        let p = unreachable_server();
        let e = install_server(&p, &item(&"a".repeat(64), 100, ""), &mut |_, _| {}).unwrap_err();
        // 没烘入验签公钥的构建同样必须拒绝，只是理由不同 —— 两种都绝不能放行
        assert!(
            e.contains("未签名") || e.contains("未烘入"),
            "未签名的插件必须被拒绝：{e}"
        );
        assert!(!e.contains("连接"), "必须在联网之前就拒绝：{e}");
    }

    /// 签名对不上必须拒绝，且待签字节由**本地**组装（服务端只能给签名本体）。
    #[test]
    fn install_rejects_signature_for_another_plugin() {
        use libsm::sm2::signature::SigCtx;
        let Some(builtin) = release::builtin_pubkey() else {
            eprintln!("跳过：本构建未烘入验签公钥");
            return;
        };
        let ctx = SigCtx::new();
        let (pk, sk) = ctx.new_keypair().unwrap();
        // 这把密钥不是烘入的那把 —— 相当于攻击者自签
        assert_ne!(
            hex::encode(ctx.serialize_pubkey(&pk, false).unwrap()),
            builtin
        );
        let sha = "a".repeat(64);
        let payload = release::plugin_signing_payload("demo", "1.0.0", 1, &sha, 100);
        let sig = hex::encode(ctx.sign(&payload, &sk, &pk).unwrap().der_encode());

        let e = install_server(
            &unreachable_server(),
            &item(&sha, 100, &sig),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(e.contains("签名"), "自签的插件必须被拒绝：{e}");
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

        let items = browse_server(&profile, "").expect("拉市场列表失败");
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
        assert!(
            !it.signature.is_empty(),
            "上架的插件必须带离线签名，否则客户端一律拒装"
        );

        // 安装：验签 → 换票 → 下载 → sha256 校验 → wasm 魔数 → 落盘
        install_server(&profile, it, &mut |_, _| {}).expect("安装失败");

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
        let updatable = check_updates_server(&profile).expect("检查更新失败");
        assert!(
            !updatable.contains(&"url-codec".to_owned()),
            "刚装的就是最新版，不该报有更新：{updatable:?}"
        );

        // sha256 对不上必须拒绝（把 sha 改一位）。
        // 注意此时**先挂掉的是验签** —— 签名覆盖了 sha256，改一位签名就对不上了，
        // 这正是我们要的：篡改在联网之前就被发现。
        let mut bad = it.sha256.clone();
        let last = bad.pop().unwrap();
        bad.push(if last == 'a' { 'b' } else { 'a' });
        let e = install_server(
            &profile,
            &MarketItem {
                sha256: bad,
                ..it.clone()
            },
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(
            e.contains("签名") || e.contains("校验失败"),
            "篡改必须拒绝：{e}"
        );

        // 冒名顶替：拿这个版本的合法签名去装成另一个 slug —— 清单绑了 slug，必须失败
        let e = install_server(
            &profile,
            &MarketItem {
                slug: "not-url-codec".into(),
                ..it.clone()
            },
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(e.contains("签名"), "换 slug 必须验签失败：{e}");

        let _ = plugin_host::uninstall("url-codec");
        eprintln!("插件市场全链路通过");
    }
}
