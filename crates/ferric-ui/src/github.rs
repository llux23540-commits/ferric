//! 把 **GitHub Releases** 当作更新源。
//!
//! # 为什么可以（以及为什么不动信任根）
//!
//! 客户端唯一据以**授权执行**的东西是离线签名：待签字节由本地按已知字段组装，
//! 用编译期烘入的 `FERRIC_RELEASE_PUBKEY` 验。这跟包从哪下来毫无关系 ——
//! 所以换成 GitHub 之后，`release::verify` 那一关一个字节都不用改。
//!
//! 传输层反而更强：自建服务端**没有 TLS**（安全性靠 SM2 信道 + 离线签名），
//! 而 GitHub 有真 TLS + 公信 CA + 全球 CDN，还免服务器、免备案。
//!
//! # 为什么走「固定下载 URL」而不是 REST API
//!
//! `https://github.com/{repo}/releases/latest/download/<文件名>` 是**普通文件下载**，
//! 不计入 API 限流；而未认证的 REST API 只有 **60 次/小时/IP**。公司 NAT 后面
//! 几十个用户共用一个出口 IP，用 `api.github.com` 迟早集体撞墙 —— 那时客户端只会
//! 显示「检查失败」，用户什么也做不了。
//!
//! # manifest.json：GitHub 给不了的东西放这里
//!
//! Release 里没有 `build`（客户端比新旧靠它）、没有每个平台的 sha256、更没有签名。
//! 所以发版时额外传一个 `manifest.json` 附件：
//!
//! ```json
//! {
//!   "version": "0.2.17", "build": 74, "notes": "……",
//!   "min_supported_build": 0,
//!   "artifacts": [
//!     { "platform": "windows", "arch": "aarch64", "ext": ".exe",
//!       "file": "ferric-v0.2.17-windows-aarch64-setup.exe",
//!       "sha256": "…", "size": 12345, "signature": "3045…" }
//!   ]
//! }
//! ```
//!
//! **流程是「先下小文件 → 验签 → 再下大文件」**：manifest 里每个条目的签名覆盖了
//! version/build/platform/arch/sha256/size，与自建服务端用的是**同一个
//! `release::signing_payload`**。所以一个被改过的 manifest 连下载都不会开始。
//!
//! # 跟随重定向：只在这条路上放开
//!
//! `net::agent()` 写死了 `.redirects(0)`，理由是「302 到别的 host 会把一次性下载
//! 票据泄露出去」。GitHub 这条路没有票据（URL 是公开的），而 `/releases/latest/download/`
//! **必然**跳到 `objects.githubusercontent.com`，不跟随就下不下来。
//! 所以这里用一个**独立的 agent**放开跳转 —— 自建服务端那条路的防线保持原样。

use crate::release;
use crate::updater::{ext_allowed, fresh_update_dir, platform_arch, ReleaseInfo};
use std::io::{Read, Write};
use std::time::Duration;

/// manifest.json 的体积上限。它只是一份小清单，几 KB 顶天；
/// 给出上限是防「对面返回一个无限大的响应把内存吃光」。
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// 安装包体积上限，与服务端 `VERSION_MAX_BYTES` 一致。
const MAX_ASSET_BYTES: u64 = 200 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 允许跟随的重定向次数。GitHub 正常是 1~2 跳，给 5 已经很宽。
const MAX_REDIRECTS: u32 = 5;

/// 一个 GitHub 更新源，就是 `owner/repo`。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubSource {
    /// 形如 `owner/repo`
    pub repo: String,
}

impl GithubSource {
    /// 编译期烘入的默认仓库（`FERRIC_GITHUB_REPO`）。未配置则返回 None。
    pub fn builtin() -> Option<Self> {
        let r = env!("FERRIC_GITHUB_REPO").trim();
        (!r.is_empty()).then(|| Self { repo: r.to_owned() })
    }

    /// 是不是编译期烘入的那个 —— 只有它才允许自动下载并执行安装包。
    pub fn is_builtin(&self) -> bool {
        Self::builtin().is_some_and(|b| &b == self)
    }

    /// 形状校验。**必须在拼 URL 之前做完**：`repo` 来自用户输入，
    /// 放进 URL 之前要挡掉 `../`、`@`、以及任何能把请求引到别的 host 的写法。
    pub fn validate(&self) -> Result<(), String> {
        let r = self.repo.trim();
        if r.is_empty() {
            return Err("仓库不能为空".into());
        }
        if r.len() > 128 {
            return Err("仓库名过长".into());
        }
        let mut parts = r.split('/');
        let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err("须为 owner/repo 形式（正好一个斜杠）".into());
        };
        for (what, seg) in [("owner", owner), ("repo", name)] {
            if seg.is_empty() {
                return Err(format!("{what} 不能为空"));
            }
            // GitHub 的合法字符集：字母数字 . _ -。放行任何别的字符都可能
            // 让它变成一个指向别处的 URL
            if !seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err(format!("{what} 含非法字符（只允许字母数字与 . _ -）"));
            }
            if seg.starts_with('.') {
                return Err(format!("{what} 不能以点开头"));
            }
        }
        Ok(())
    }

    /// 展示用的短标识。
    pub fn label(&self) -> String {
        format!("GitHub · {}", self.repo.trim())
    }

    fn asset_url(&self, file: &str) -> String {
        format!(
            "https://github.com/{}/releases/latest/download/{file}",
            self.repo.trim()
        )
    }
}

/// 跟随重定向的 agent。**只给 GitHub 这条路用**，理由见模块头部。
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .redirects(MAX_REDIRECTS)
        .build()
}

/// manifest 里的一个平台条目（已经按本机平台/架构筛选过，故不再重复存那两项）。
struct Artifact {
    ext: String,
    file: String,
    sha256: String,
    size: i64,
    signature: String,
}

/// 检查更新。返回 `Ok(None)` 表示确实已是最新。
///
/// 与服务端那条路一样，**新旧由本地重算**：manifest 里就算写着什么
/// `has_update` 也不看，只比 build。
pub fn check(src: &GithubSource) -> Result<Option<ReleaseInfo>, String> {
    src.validate()?;
    let (platform, arch) = platform_arch()?;
    let Some(verify_pk) = release::builtin_pubkey() else {
        return Err("本构建未烘入发布验签公钥，无法验证安装包来源".into());
    };
    let body = fetch_text(&src.asset_url("manifest.json"), MAX_MANIFEST_BYTES)?;
    parse_manifest(&body, platform, arch, crate::updater::my_build(), verify_pk)
}

/// 解析并**验签** manifest，挑出适合本机的那一条。
///
/// 单独拆出来是为了能测：整个 GitHub 分支的判断力都集中在这里，而网络那层只是
/// 「把一段文本取回来」。参数里显式传 `my_build` 与 `verify_pk`，
/// 让测试能注入自己的密钥与版本号，不必依赖编译期烘入值。
fn parse_manifest(
    body: &str,
    platform: &str,
    arch: &str,
    my_build: i64,
    verify_pk: &str,
) -> Result<Option<ReleaseInfo>, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("manifest.json 不是合法 JSON：{e}"))?;

    let version = v["version"].as_str().unwrap_or_default().to_owned();
    let build = v["build"].as_i64().unwrap_or_default();
    if version.is_empty() || build <= 0 {
        return Err("manifest 缺少 version / build".into());
    }
    if build <= my_build {
        return Ok(None); // 已是最新
    }

    let Some(a) = pick_artifact(&v, platform, arch) else {
        // 该平台这次没发包 —— 是「没有适合你的更新」，不是错误
        return Ok(None);
    };
    if !ext_allowed(platform, &a.ext) {
        return Err(format!("不接受的安装包类型：{}", a.ext));
    }
    if a.sha256.len() != 64 || a.size <= 0 {
        return Err("manifest 里的 sha256 / size 不合法".into());
    }
    if a.signature.trim().is_empty() {
        return Err("该版本未签名，已拒绝（签名是防止发布账号被拿下的唯一手段）".into());
    }

    // ⚠️ 验签放在**下载之前**：待签字节全部由本地按已知字段组装，
    // GitHub 只提供签名本身。manifest 被改过的话，这里就断了。
    let payload = release::signing_payload(&version, build, platform, arch, &a.sha256, a.size);
    release::verify(verify_pk, &a.signature, &payload)
        .map_err(|e| format!("manifest 签名校验不通过：{e}"))?;

    let min_supported_build = v["min_supported_build"].as_i64().unwrap_or(0);
    Ok(Some(ReleaseInfo {
        // GitHub 这条路没有数据库主键，也不需要换票；用 0 占位。
        // 文件名不进 ReleaseInfo —— 下载时会重新取一次 manifest 并重新验签，
        // 期望值绝不从上一次的界面状态里捞
        id: 0,
        version,
        build,
        notes: v["notes"].as_str().unwrap_or_default().to_owned(),
        sha256: a.sha256,
        size: a.size,
        min_supported_build,
        signature: a.signature,
        ext: a.ext,
        // 与服务端一致：force 由本地重算，不采信对面写的
        force: my_build < min_supported_build && min_supported_build <= build,
    }))
}

/// 从 manifest 里挑出本平台的条目。
fn pick_artifact(v: &serde_json::Value, platform: &str, arch: &str) -> Option<Artifact> {
    v["artifacts"].as_array()?.iter().find_map(|it| {
        (it["platform"].as_str()? == platform && it["arch"].as_str()? == arch).then(|| Artifact {
            ext: it["ext"].as_str().unwrap_or_default().to_owned(),
            file: it["file"].as_str().unwrap_or_default().to_owned(),
            sha256: it["sha256"].as_str().unwrap_or_default().to_owned(),
            size: it["size"].as_i64().unwrap_or_default(),
            signature: it["signature"].as_str().unwrap_or_default().to_owned(),
        })
    })
}

/// 下载安装包并做完三重校验，返回可执行的文件路径。
pub fn download(
    src: &GithubSource,
    info: &ReleaseInfo,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<std::path::PathBuf, String> {
    src.validate()?;
    let (platform, arch) = platform_arch()?;
    let Some(verify_pk) = release::builtin_pubkey() else {
        return Err("本构建未烘入发布验签公钥，无法验证安装包来源".into());
    };

    // 重新取一次 manifest 拿文件名：`check` 与 `download` 之间可能隔了很久，
    // 而且**必须重新验签** —— 期望值绝不能从上一次的界面状态里捞。
    let body = fetch_text(&src.asset_url("manifest.json"), MAX_MANIFEST_BYTES)?;
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("manifest.json 不是合法 JSON：{e}"))?;
    let a = pick_artifact(&v, platform, arch).ok_or("该平台没有可下载的安装包")?;
    if a.sha256 != info.sha256 || a.size != info.size {
        return Err("清单已变化（可能刚发了新版），请重新检查更新".into());
    }
    if a.file.is_empty() || a.file.contains(['/', '\\']) || a.file.contains("..") {
        return Err("manifest 里的文件名非法".into());
    }

    let dir = fresh_update_dir()?;
    let file = dir.join(format!(
        "ferric-{}-{}-{}{}",
        info.version,
        info.build,
        &info.sha256[..info.sha256.len().min(16)],
        info.ext
    ));

    let fail = |dir: &std::path::Path, msg: String| -> String {
        let _ = std::fs::remove_dir_all(dir);
        msg
    };
    if let Err(e) = stream_to_file(
        &src.asset_url(&a.file),
        &file,
        info.size as u64,
        on_progress,
    ) {
        return Err(fail(&dir, e));
    }
    on_progress(info.size as u64, info.size as u64);

    // 与自建服务端**共用同一个校验函数**，绝不另写一份
    crate::updater::verify_downloaded(&dir, &file, info, platform, arch, verify_pk)?;
    Ok(file)
}

/// 取一个小文本文件（manifest）。
fn fetch_text(url: &str, max: u64) -> Result<String, String> {
    let resp = agent().get(url).call().map_err(|e| match e {
        ureq::Error::Status(404, _) => {
            "该仓库的最新发布里没有 manifest.json（发布流程需要上传它）".to_owned()
        }
        ureq::Error::Status(code, _) => format!("GitHub 返回 HTTP {code}"),
        e => format!("连接 GitHub 失败：{e}"),
    })?;
    let mut s = String::new();
    resp.into_reader()
        .take(max + 1)
        .read_to_string(&mut s)
        .map_err(|e| format!("读取失败：{e}"))?;
    if s.len() as u64 > max {
        return Err("manifest.json 过大，已拒绝".into());
    }
    Ok(s)
}

/// 边下边写。**不做流式哈希** —— 要校验的必须是「将要被执行的那串字节」，
/// 由 `verify_downloaded` 从磁盘重新读一遍算（与服务端那条路同样的理由）。
fn stream_to_file(
    url: &str,
    file: &std::path::Path,
    total: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    if total > MAX_ASSET_BYTES {
        return Err("安装包超过 200MB 上限，已拒绝".into());
    }
    let resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("下载失败：{e}"))?;

    // create_new：已存在的文件或符号链接直接失败，不跟随
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .map_err(|e| format!("创建临时文件失败：{e}"))?;

    let mut reader = resp.into_reader().take(MAX_ASSET_BYTES + 1);
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut last = std::time::Instant::now();
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("读取失败：{e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .map_err(|e| format!("写入失败：{e}"))?;
        done += n as u64;
        if done > MAX_ASSET_BYTES {
            return Err("下载内容超过上限，已中止".into());
        }
        // 进度节流：每块都上报会把 channel 塞爆，也会把界面拖进重绘风暴
        if last.elapsed() >= crate::updater::PROGRESS_BEAT {
            on_progress(done, total);
            last = std::time::Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `repo` 会被直接拼进 URL，形状校验是唯一的守卫。
    #[test]
    fn repo_shape_is_strictly_validated() {
        let ok = |r: &str| GithubSource { repo: r.to_owned() }.validate();
        assert!(ok("llux23540-commits/ferric").is_ok());
        assert!(ok("a/b").is_ok());
        assert!(ok("Some.Org/my_repo-2").is_ok());

        for bad in [
            "",                    // 空
            "ferric",              // 缺斜杠
            "a/b/c",               // 多一段
            "/ferric",             // owner 空
            "owner/",              // repo 空
            "owner/../../etc",     // 路径穿越
            "owner/repo?x=1",      // query 注入
            "owner/repo#frag",     // 片段
            "evil.com/owner/repo", // 想换 host
            "owner@host/repo",     // 凭证形式
            "owner/re po",         // 空格
            "owner/.hidden",       // 点开头
            "owner/repo\\..\\x",   // 反斜杠
        ] {
            assert!(ok(bad).is_err(), "「{bad}」应被拒绝");
        }
    }

    /// URL 必须落在 github.com 上，且用的是**不计 API 限流**的固定下载路径。
    #[test]
    fn asset_url_is_the_non_api_permalink() {
        let s = GithubSource {
            repo: "owner/repo".into(),
        };
        assert_eq!(
            s.asset_url("manifest.json"),
            "https://github.com/owner/repo/releases/latest/download/manifest.json"
        );
        assert!(
            !s.asset_url("x").contains("api.github.com"),
            "绝不能走 REST API —— 未认证只有 60 次/小时/IP"
        );
    }

    #[test]
    fn picks_only_the_matching_platform() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"artifacts":[
                {"platform":"linux","arch":"x86_64","ext":".deb","file":"a.deb","sha256":"aa","size":1,"signature":"30"},
                {"platform":"windows","arch":"aarch64","ext":".exe","file":"b.exe","sha256":"bb","size":2,"signature":"31"}
            ]}"#,
        )
        .unwrap();
        let a = pick_artifact(&v, "windows", "aarch64").expect("应挑中 windows/aarch64");
        assert_eq!(a.file, "b.exe");
        assert_eq!(a.ext, ".exe");
        // 架构不匹配的绝不能被当成「差不多」拿去装
        assert!(pick_artifact(&v, "windows", "x86_64").is_none());
        assert!(pick_artifact(&v, "macos", "aarch64").is_none());
    }

    // ---- manifest 解析 + 验签：整个 GitHub 分支的判断力都在这儿 ----

    use libsm::sm2::signature::SigCtx;

    /// 造一对发布密钥，返回 (公钥hex, 私钥hex)。
    fn keypair() -> (String, String) {
        let ctx = SigCtx::new();
        let (pk, sk) = ctx.new_keypair().unwrap();
        (
            hex::encode(ctx.serialize_pubkey(&pk, false).unwrap()),
            hex::encode(ctx.serialize_seckey(&sk).unwrap()),
        )
    }

    fn sign(sk_hex: &str, payload: &[u8]) -> String {
        let ctx = SigCtx::new();
        let sk = ctx.load_seckey(&hex::decode(sk_hex).unwrap()).unwrap();
        let pk = ctx.pk_from_sk(&sk).unwrap();
        hex::encode(ctx.sign(payload, &sk, &pk).unwrap().der_encode())
    }

    const SHA: &str = "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12";

    /// 造一份签好名的 manifest（build=100, windows/aarch64, 1234 字节）。
    fn signed_manifest(sk: &str, min_supported_build: i64) -> String {
        let payload = release::signing_payload("9.9.9", 100, "windows", "aarch64", SHA, 1234);
        let sig = sign(sk, &payload);
        format!(
            r#"{{"version":"9.9.9","build":100,"notes":"修了三处卡顿",
                 "min_supported_build":{min_supported_build},
                 "artifacts":[{{"platform":"windows","arch":"aarch64","ext":".exe",
                   "file":"ferric-setup.exe","sha256":"{SHA}","size":1234,
                   "signature":"{sig}"}}]}}"#
        )
    }

    #[test]
    fn valid_manifest_yields_an_update() {
        let (pk, sk) = keypair();
        let info = parse_manifest(&signed_manifest(&sk, 0), "windows", "aarch64", 50, &pk)
            .expect("应解析成功")
            .expect("build 100 > 50，应报告有更新");
        assert_eq!(info.version, "9.9.9");
        assert_eq!(info.build, 100);
        assert_eq!(info.sha256, SHA);
        assert_eq!(info.ext, ".exe");
        assert_eq!(info.notes, "修了三处卡顿");
        assert!(!info.force);
    }

    /// 已经是最新（或更新）时必须是 `Ok(None)`，**不是错误** ——
    /// 报错会在界面上挂一条「检查失败」，用户会以为更新坏了。
    #[test]
    fn same_or_newer_build_is_up_to_date_not_an_error() {
        let (pk, sk) = keypair();
        let m = signed_manifest(&sk, 0);
        assert!(parse_manifest(&m, "windows", "aarch64", 100, &pk)
            .unwrap()
            .is_none());
        assert!(parse_manifest(&m, "windows", "aarch64", 999, &pk)
            .unwrap()
            .is_none());
    }

    /// 本平台这次没发包，同样是「没有适合你的更新」而不是错误。
    #[test]
    fn missing_platform_is_up_to_date_not_an_error() {
        let (pk, sk) = keypair();
        let m = signed_manifest(&sk, 0);
        assert!(parse_manifest(&m, "linux", "x86_64", 1, &pk)
            .unwrap()
            .is_none());
        // 架构对不上也一样 —— 绝不能「差不多就装」
        assert!(parse_manifest(&m, "windows", "x86_64", 1, &pk)
            .unwrap()
            .is_none());
    }

    /// **被改过的 manifest 连下载都不该开始。** 逐字段改一处，验签就必须断。
    #[test]
    fn any_tampering_breaks_the_signature() {
        let (pk, sk) = keypair();
        let good = signed_manifest(&sk, 0);

        for (what, tampered) in [
            ("版本号", good.replace("9.9.9", "9.9.8")),
            ("构建号", good.replace("\"build\":100", "\"build\":101")),
            ("大小", good.replace("1234", "1235")),
            ("sha256", good.replace(SHA, &"cd".repeat(32))),
        ] {
            let r = parse_manifest(&tampered, "windows", "aarch64", 50, &pk);
            assert!(r.is_err(), "改了{what}居然还能通过");
            assert!(
                r.unwrap_err().contains("签名"),
                "错误信息要指明是签名的问题（{what}）"
            );
        }

        // 换一把私钥签的同样不行
        let (_, other_sk) = keypair();
        let forged = signed_manifest(&other_sk, 0);
        assert!(parse_manifest(&forged, "windows", "aarch64", 50, &pk).is_err());
    }

    /// 没有签名字段 = 未背书，一律拒绝；不能当成「暂时先信一下」。
    #[test]
    fn unsigned_artifact_is_rejected() {
        let (pk, sk) = keypair();
        let m = signed_manifest(&sk, 0);
        let re = regex_lite_replace_signature(&m);
        let err = parse_manifest(&re, "windows", "aarch64", 50, &pk).unwrap_err();
        assert!(err.contains("未签名"), "{err}");
    }

    /// 把 signature 的值换成空串（不引正则依赖，手工找一次即可）。
    fn regex_lite_replace_signature(m: &str) -> String {
        let key = "\"signature\":\"";
        let start = m.find(key).unwrap() + key.len();
        let end = start + m[start..].find('"').unwrap();
        format!("{}{}", &m[..start], &m[end..])
    }

    /// 畸形输入只能得到错误，不能 panic —— manifest 是从网上取回来的外部输入。
    #[test]
    fn malformed_manifest_never_panics() {
        let (pk, _) = keypair();
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"version":"1.0.0"}"#,
            r#"{"version":"","build":9}"#,
            r#"{"version":"1.0.0","build":-1}"#,
            r#"{"version":"1.0.0","build":9,"artifacts":"nope"}"#,
            r#"{"version":"1.0.0","build":9,"artifacts":[{"platform":"windows"}]}"#,
        ] {
            let _ = parse_manifest(bad, "windows", "aarch64", 1, &pk);
        }
    }

    /// 平台与扩展名必须配对：windows 收到 .AppImage 要当场拒绝，
    /// 不能让一个不可能执行的文件走到下载那一步。
    #[test]
    fn extension_must_match_the_platform() {
        let (pk, sk) = keypair();
        let m = signed_manifest(&sk, 0).replace("\".exe\"", "\".AppImage\"");
        let err = parse_manifest(&m, "windows", "aarch64", 50, &pk).unwrap_err();
        assert!(err.contains("不接受的安装包类型"), "{err}");
    }

    /// 强制更新标记由**本地重算**，manifest 说了不算。
    #[test]
    fn force_is_recomputed_locally() {
        let (pk, sk) = keypair();
        let m = signed_manifest(&sk, 80);
        // 本机 build 50 < 最低支持 80 → 强制
        assert!(
            parse_manifest(&m, "windows", "aarch64", 50, &pk)
                .unwrap()
                .unwrap()
                .force
        );
        // 本机 build 90 已经达标 → 不强制
        assert!(
            !parse_manifest(&m, "windows", "aarch64", 90, &pk)
                .unwrap()
                .unwrap()
                .force
        );
    }

    #[test]
    fn builtin_flag_tracks_the_baked_repo() {
        let custom = GithubSource {
            repo: "someone-else/fork".into(),
        };
        // 未烘入时任何仓库都不是内置的；烘入了也只有那一个是
        assert_eq!(custom.is_builtin(), GithubSource::builtin() == Some(custom));
    }
}
