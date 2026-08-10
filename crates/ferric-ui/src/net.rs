//! 与 ferric-server 通信的国密传输层（SM2 数字信封 + SM4-GCM 报文加密）。
//!
//! # 线格式（与服务端 `server/src/crypto.rs` 严格一致）
//!
//! 请求头 `X-Enc: sm2-sm4-gcm` / `X-Enc-Key: <SM2(会话密钥)>` / `X-Enc-Iv: <16字节hex>`，
//! 请求体与响应体都是 `{"d":"<密文hex>","t":"<tag hex>"}`，响应 IV 在响应头 `X-Enc-Iv`。
//!
//! 两端都用 **libsm**，所以浏览器那边踩过的三个跨实现坑（C1C2C3 顺序、C1 的 `04` 前缀、
//! SM4-GCM 的 16 字节 IV）在这里不存在 —— 但仍然钉了固定测试向量，防止将来有人换实现。
//!
//! # 这一层如何保证「对面就是我指定的服务」
//!
//! 会话密钥用**编译期烘入的**服务端公钥包裹，只有持有对应私钥的真服务端能解开；
//! 响应用同一把会话密钥做 SM4-GCM，中间人拿不到密钥就伪造不出能通过 tag 校验的响应。
//! 于是「能跟我把话说通」本身就是身份证明。附带两个性质：
//! 会话密钥一次一换 ⇒ 重放旧响应必然解密失败；拒绝明文响应 ⇒ 堵死降级攻击。
//!
//! # 调 libsm 之前必须完成全部守卫
//!
//! libsm 的解密路径**没有边界检查**：`DecryptCtx::decrypt` 直接切 `cipher[0..65]`，
//! `gcm_decrypt` 直接切 `data[data.len()-16..]`。一个畸形响应就能 panic 掉整个客户端。
//! 所以下面每个入口都先做 hex/长度校验，通过了才喂给 libsm。

use libsm::sm2::encrypt::EncryptCtx;
use libsm::sm2::signature::SigCtx;
use libsm::sm4::{Cipher, Mode};
use rand::RngCore;
use std::io::Read;
use std::time::Duration;

pub const ALG: &str = "sm2-sm4-gcm";
const SESSION_KEY_LEN: usize = 16;
const IV_LEN: usize = 16;
const TAG_LEN: usize = 16;
/// 响应体上限。更新检查的 JSON 都很小，给足余量即可，防止服务端/中间人灌大响应。
const MAX_RESP_BYTES: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum NetErr {
    /// 网络层失败（连不上、超时、非 2xx 等）
    Transport(String),
    /// 对方没按加密协议说话 —— 一律当作「检查失败」，**绝不能降级成「已是最新」**
    NotEncrypted,
    /// 解密或完整性校验失败（密钥不对 / 报文被篡改 / 对面不是真服务端）
    Crypto(String),
    /// 服务端返回了业务错误（明文错误信封）
    Server(String),
    /// 响应结构对不上
    Malformed(String),
}

impl std::fmt::Display for NetErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "网络请求失败：{e}"),
            Self::NotEncrypted => write!(f, "服务端未按加密协议应答，已拒绝（可能存在中间人）"),
            Self::Crypto(e) => write!(f, "响应校验失败：{e}"),
            Self::Server(e) => write!(f, "{e}"),
            Self::Malformed(e) => write!(f, "响应格式异常：{e}"),
        }
    }
}

/// 服务器身份 = 地址 + 公钥，**必须作为一个整体**存取。
///
/// 分开存会让「只改 URL、不改公钥」成为可能，那正是最容易被利用的一种改法。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServerProfile {
    pub base_url: String,
    /// SM2 未压缩公钥 hex（`04` 开头 130 字符）
    pub pubkey: String,
}

impl ServerProfile {
    /// 编译期烘入的默认服务器（见 `build.rs` 的 `bake_update_pins`）。
    /// 未配置时返回 None —— 此时更新功能禁用，**绝不回落到向服务端索取公钥**。
    pub fn builtin() -> Option<Self> {
        let base_url = env!("FERRIC_SERVER_URL").trim();
        let pubkey = env!("FERRIC_SERVER_PUBKEY").trim();
        if base_url.is_empty() || pubkey.is_empty() {
            return None;
        }
        Some(Self {
            base_url: base_url.to_owned(),
            pubkey: pubkey.to_owned(),
        })
    }

    /// 公钥格式自检：必须是未压缩格式且确实是曲线上的点。
    /// 在配置入口就查出来，而不是等到每次检查更新都报「解密失败」。
    pub fn validate(&self) -> Result<(), String> {
        if !(self.base_url.starts_with("http://") || self.base_url.starts_with("https://")) {
            return Err("服务器地址必须以 http:// 或 https:// 开头".into());
        }
        if self.base_url.contains('@') {
            return Err("服务器地址不得包含凭证".into());
        }
        let bytes = hex::decode(&self.pubkey).map_err(|_| "公钥不是合法 hex".to_owned())?;
        if bytes.len() != 65 || bytes[0] != 0x04 {
            return Err("公钥须为未压缩格式（04 开头，130 个 hex 字符）".into());
        }
        SigCtx::new()
            .load_pubkey(&bytes)
            .map_err(|_| "公钥不是 SM2 曲线上的点".to_owned())?;
        Ok(())
    }

    /// 公钥指纹，给人核对用（SHA-256 前 8 字节，四字一组）。
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.pubkey)
    }

    /// 用户输入的指纹是否与本服务器一致。
    ///
    /// 核对指纹的场景就是「从网页上抄一串到客户端里」，中间必然掺进空格、大小写、
    /// 甚至换行，所以比较前要**先归一化**。反过来，归一化只能去掉这些噪声，
    /// 绝不能做「前缀匹配」「忽略若干位」之类的宽松处理 —— 指纹的全部价值就在于
    /// 逐位相同，放宽一位就等于把攻击者要碰撞的位数减少一位。
    pub fn fingerprint_matches(&self, input: &str) -> bool {
        let norm = |s: &str| -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_uppercase())
                .collect()
        };
        let expected = norm(&self.fingerprint());
        let got = norm(input);
        !got.is_empty() && got == expected
    }

    /// 是不是编译期烘入的那个。不是的话，自动安装会被禁用（见 `updater`）。
    pub fn is_builtin(&self) -> bool {
        Self::builtin().is_some_and(|b| &b == self)
    }
}

/// 由公钥算指纹。**算的是 hex 字符串本身的 SHA-256**，不是解码后的字节 ——
/// 服务端 `crypto.rs` 与后台页面用的是同一套算法，改这里必须同步改那边，
/// 否则运维照着后台念的指纹和用户在客户端里看到的对不上。
pub fn fingerprint_of(pubkey_hex: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(pubkey_hex.as_bytes());
    hex::encode(&d[..8])
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 一次请求的会话密钥。**每个请求都新生成，禁止缓存复用** ——
/// 复用会同时牺牲重放保护，并诱使实现者去用计数器 IV。
struct Session {
    key: Vec<u8>,
    key_header: String,
}

fn new_session(pubkey_hex: &str) -> Result<Session, NetErr> {
    let mut key = vec![0u8; SESSION_KEY_LEN];
    rand::thread_rng().fill_bytes(&mut key);

    let pk_bytes =
        hex::decode(pubkey_hex).map_err(|_| NetErr::Crypto("服务端公钥不是合法 hex".into()))?;
    let pk = SigCtx::new()
        .load_pubkey(&pk_bytes)
        .map_err(|_| NetErr::Crypto("服务端公钥非法".into()))?;
    // ⚠️ klen 必须与明文长度**完全相等**：libsm 在 klen > 明文长度时会 panic，
    //    klen < 明文长度时会静默截断。
    let ct = EncryptCtx::new(SESSION_KEY_LEN, pk)
        .encrypt(&key)
        .map_err(|_| NetErr::Crypto("会话密钥封装失败".into()))?;
    Ok(Session {
        key,
        key_header: hex::encode(ct), // libsm 自带 04 前缀，无需手工补
    })
}

fn seal(key: &[u8], plain: &[u8]) -> Result<(String, String, String), NetErr> {
    let mut iv = [0u8; IV_LEN];
    rand::thread_rng().fill_bytes(&mut iv);
    let cipher =
        Cipher::new(key, Mode::Gcm).map_err(|_| NetErr::Crypto("SM4 初始化失败".into()))?;
    let out = cipher
        .encrypt(&[], plain, &iv)
        .map_err(|_| NetErr::Crypto("SM4-GCM 加密失败".into()))?;
    if out.len() < TAG_LEN {
        return Err(NetErr::Crypto("加密输出长度异常".into()));
    }
    let (d, t) = out.split_at(out.len() - TAG_LEN);
    Ok((hex::encode(iv), hex::encode(d), hex::encode(t)))
}

/// 解密响应。**全部守卫在调 libsm 之前完成** —— 详见模块头部说明。
fn open(key: &[u8], iv_hex: &str, d_hex: &str, t_hex: &str) -> Result<Vec<u8>, NetErr> {
    let iv = hex::decode(iv_hex.trim()).map_err(|_| NetErr::Crypto("IV 不是合法 hex".into()))?;
    if iv.len() != IV_LEN {
        return Err(NetErr::Crypto("IV 长度非法".into()));
    }
    let tag = hex::decode(t_hex.trim()).map_err(|_| NetErr::Crypto("tag 不是合法 hex".into()))?;
    if tag.len() != TAG_LEN {
        return Err(NetErr::Crypto("tag 长度非法".into()));
    }
    let mut buf =
        hex::decode(d_hex.trim()).map_err(|_| NetErr::Crypto("密文不是合法 hex".into()))?;
    buf.extend_from_slice(&tag); // libsm 期望 tag 拼在密文尾部
    let cipher = Cipher::new(key, Mode::Gcm).map_err(|_| NetErr::Crypto("会话密钥非法".into()))?;
    cipher
        .decrypt(&[], &buf, &iv)
        .map_err(|_| NetErr::Crypto("完整性校验不通过".into()))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        // 禁跟随重定向：302 到别的 host 会把一次性下载票据泄露出去，
        // 也让「固定服务器地址」这件事失去意义。
        .redirects(0)
        .build()
}

/// 发一个加密请求并解密响应。
///
/// `path` 是相对 `profile.base_url` 的路径，**永远不接受绝对 URL** ——
/// 服务端返回的 `download_url` 之类只当路径用，绝不能让它把请求引到别的 host。
pub fn call(
    profile: &ServerProfile,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, NetErr> {
    if !path.starts_with('/') {
        return Err(NetErr::Malformed("路径必须以 / 开头".into()));
    }
    let session = new_session(&profile.pubkey)?;
    let url = format!("{}{}", profile.base_url.trim_end_matches('/'), path);

    let mut req = agent()
        .request(method, &url)
        .set("X-Enc", ALG)
        .set("X-Enc-Key", &session.key_header);

    let resp = match body {
        None => {
            // GET / DELETE 无请求体：信封只用于协商响应密钥，IV 占位
            req = req.set("X-Enc-Iv", &"0".repeat(IV_LEN * 2));
            req.call()
        }
        Some(v) => {
            let (iv, d, t) = seal(&session.key, v.to_string().as_bytes())?;
            req = req
                .set("X-Enc-Iv", &iv)
                .set("Content-Type", "application/json");
            req.send_string(&serde_json::json!({ "d": d, "t": t }).to_string())
        }
    };

    // ureq 把非 2xx 归到 Err(Status)；服务端的业务错误也走这条路
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            // 中间件自身的错误（缺信封、解密失败）是**明文**的，这里能读出可读原因
            let text = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("msg").and_then(|m| m.as_str()).map(str::to_owned))
                .unwrap_or_else(|| format!("HTTP {code}"));
            return Err(NetErr::Server(msg));
        }
        Err(e) => return Err(NetErr::Transport(e.to_string())),
    };

    // ⚠️ 判定顺序不可协商。服务端会**用明文返回自己产生的错误**，所以「收到明文响应」
    //    在本系统里是正常现象而非异常 —— 若写成「先试着解析 JSON，失败再看 X-Enc」，
    //    一个明文的 {"has_update":false} 就能把客户端骗过去。
    if resp.header("X-Enc").map(str::trim) != Some(ALG) {
        return Err(NetErr::NotEncrypted);
    }
    let iv = resp
        .header("X-Enc-Iv")
        .map(str::to_owned)
        .ok_or(NetErr::NotEncrypted)?;

    let mut raw = Vec::new();
    resp.into_reader()
        .take(MAX_RESP_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| NetErr::Transport(e.to_string()))?;
    if raw.len() > MAX_RESP_BYTES {
        return Err(NetErr::Transport("响应体过大".into()));
    }

    let env: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|_| NetErr::Malformed("响应不是 {d,t} 信封".into()))?;
    let (Some(d), Some(t)) = (
        env.get("d").and_then(|v| v.as_str()),
        env.get("t").and_then(|v| v.as_str()),
    ) else {
        return Err(NetErr::Malformed("响应缺少 d/t 字段".into()));
    };

    // 只有解密成功的字节才允许被 JSON 解析
    let plain = open(&session.key, &iv, d, t)?;
    serde_json::from_slice(&plain).map_err(|e| NetErr::Malformed(e.to_string()))
}

/// 下载二进制（明文，靠一次性票据鉴权 + 加密信道拿到的 sha256 校验完整性）。
/// 流式写入 `sink`，边写边由调用方算哈希；`max_bytes` 是硬上限，防止被灌爆磁盘。
pub fn download_to(
    profile: &ServerProfile,
    path_with_ticket: &str,
    max_bytes: u64,
    sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<u64, NetErr> {
    if !path_with_ticket.starts_with('/') {
        return Err(NetErr::Malformed("下载路径必须以 / 开头".into()));
    }
    let url = format!(
        "{}{}",
        profile.base_url.trim_end_matches('/'),
        path_with_ticket
    );
    let resp = match agent().get(&url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => {
            return Err(NetErr::Transport(format!("下载失败 HTTP {code}")))
        }
        Err(e) => return Err(NetErr::Transport(e.to_string())),
    };

    let mut reader = resp.into_reader().take(max_bytes + 1);
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| NetErr::Transport(e.to_string()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max_bytes {
            return Err(NetErr::Transport("下载内容超过声明的大小".into()));
        }
        sink(&buf[..n]).map_err(|e| NetErr::Transport(e.to_string()))?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// seal/open 回环，且篡改任一部分都必须失败。
    #[test]
    fn seal_open_roundtrip_and_tamper_detection() {
        let mut key = vec![0u8; SESSION_KEY_LEN];
        rand::thread_rng().fill_bytes(&mut key);
        let msg = br#"{"has_update":true}"#;
        let (iv, d, t) = seal(&key, msg).unwrap();
        assert_eq!(open(&key, &iv, &d, &t).unwrap(), msg);

        let flip = |s: &str| {
            let last = &s[s.len() - 1..];
            format!(
                "{}{}",
                &s[..s.len() - 1],
                if last == "f" { "0" } else { "f" }
            )
        };
        assert!(open(&key, &iv, &d, &flip(&t)).is_err(), "篡改 tag");
        assert!(open(&key, &iv, &flip(&d), &t).is_err(), "篡改密文");
        assert!(open(&key, &flip(&iv), &d, &t).is_err(), "换 IV");
        let mut other = vec![0u8; SESSION_KEY_LEN];
        rand::thread_rng().fill_bytes(&mut other);
        assert!(open(&other, &iv, &d, &t).is_err(), "换密钥");
    }

    /// 畸形输入必须返回错误而**不是 panic** —— libsm 的解密路径没有边界检查，
    /// 这些值来自网络，一个畸形响应不能把客户端打崩。
    #[test]
    fn malformed_ciphertext_never_panics() {
        let key = vec![7u8; SESSION_KEY_LEN];
        let cases: &[(&str, &str, &str)] = &[
            ("", "", ""),
            ("zz", "aa", "bb"),
            (&"0".repeat(32), "", ""),                  // 空密文空 tag
            (&"0".repeat(32), "aabb", "cc"),            // tag 太短
            (&"0".repeat(32), "aabb", &"a".repeat(64)), // tag 太长
            ("0011", "aabb", &"0".repeat(32)),          // IV 太短
        ];
        for (iv, d, t) in cases {
            assert!(open(&key, iv, d, t).is_err(), "iv={iv} d={d} t={t}");
        }
    }

    /// 每次都必须是全新的会话密钥与 IV。复用会同时毁掉重放保护和 GCM 的安全性。
    #[test]
    fn session_key_and_iv_are_fresh() {
        let ctx = SigCtx::new();
        let (pk, _) = ctx.new_keypair().unwrap();
        let pk_hex = hex::encode(ctx.serialize_pubkey(&pk, false).unwrap());
        let a = new_session(&pk_hex).unwrap();
        let b = new_session(&pk_hex).unwrap();
        assert_ne!(a.key, b.key, "会话密钥必须一次一换");
        assert_ne!(a.key_header, b.key_header);

        let (iv1, ..) = seal(&a.key, b"same").unwrap();
        let (iv2, ..) = seal(&a.key, b"same").unwrap();
        assert_ne!(iv1, iv2, "IV 必须一次一换");
    }

    /// SM2 密文必须是 libsm 期望的 `04 ‖ C1(64) ‖ C2(16) ‖ C3(32)` = 113 字节。
    /// 这是与服务端互通的线格式锚点。
    #[test]
    fn wrapped_key_matches_wire_format() {
        let ctx = SigCtx::new();
        let (pk, _) = ctx.new_keypair().unwrap();
        let pk_hex = hex::encode(ctx.serialize_pubkey(&pk, false).unwrap());
        let s = new_session(&pk_hex).unwrap();
        let raw = hex::decode(&s.key_header).unwrap();
        assert_eq!(raw.len(), 65 + SESSION_KEY_LEN + 32, "SM2 密文长度");
        assert_eq!(raw[0], 0x04, "C1 必须带未压缩前缀");
    }

    #[test]
    fn profile_validation_rejects_bad_input() {
        let ok_pk = {
            let ctx = SigCtx::new();
            let (pk, _) = ctx.new_keypair().unwrap();
            hex::encode(ctx.serialize_pubkey(&pk, false).unwrap())
        };
        let p = |u: &str, k: &str| ServerProfile {
            base_url: u.into(),
            pubkey: k.into(),
        };
        assert!(p("http://a/api/v1", &ok_pk).validate().is_ok());
        assert!(p("ftp://a", &ok_pk).validate().is_err(), "非 http scheme");
        assert!(
            p("http://u:p@a", &ok_pk).validate().is_err(),
            "内嵌凭证的地址"
        );
        assert!(p("http://a", "zz").validate().is_err(), "非 hex 公钥");
        assert!(
            p("http://a", &"aa".repeat(65)).validate().is_err(),
            "非 04 开头"
        );
        assert!(
            p("http://a", &format!("04{}", "aa".repeat(64)))
                .validate()
                .is_err(),
            "不在曲线上的点"
        );
    }

    /// 指纹要稳定且可读（给人口头核对用）。
    #[test]
    fn fingerprint_is_stable_and_readable() {
        let p = ServerProfile {
            base_url: "http://a".into(),
            pubkey: format!("04{}", "ab".repeat(64)),
        };
        let f = p.fingerprint();
        assert_eq!(f, p.fingerprint(), "同一公钥指纹必须稳定");
        assert_eq!(f.len(), 19, "8 字节 hex 分 4 组，3 个空格");
        let other = ServerProfile {
            base_url: "http://a".into(),
            pubkey: format!("04{}", "cd".repeat(64)),
        };
        assert_ne!(f, other.fingerprint());
        // 独立函数与方法必须给出同一个结果（后台页面用的是同一套算法）
        assert_eq!(f, fingerprint_of(&p.pubkey));
    }

    /// 跨进程契约：同一把公钥，客户端与服务端必须算出**同一串**指纹。
    ///
    /// 这条一旦红了，「核对指纹」这个功能就整个失去意义 —— 运维照着后台念的和
    /// 用户在客户端里看到的对不上，用户只会学会「忽略这个提示」。
    /// 服务端 `ferric-server/server/src/crypto.rs` 里钉了**同一组向量**，
    /// 改算法必须两边同时改、两边测试都过。
    #[test]
    fn fingerprint_vector_matches_the_server() {
        const PUBKEY: &str = "04f15746ccaa2aec206b598fe47d7e7430e97b862d16817ed142f69b94c07912c38a8cfbffd19610a157483fff24c45d0af23eca41d13ab9343ea06af172899313";
        assert_eq!(fingerprint_of(PUBKEY), "2729 CD5E E522 D9FB");
    }

    /// 核对指纹：抄写过程中的噪声要容忍，**位本身一位都不能差**。
    #[test]
    fn fingerprint_matching_tolerates_transcription_noise_only() {
        let p = ServerProfile {
            base_url: "http://a".into(),
            pubkey: format!("04{}", "ab".repeat(64)),
        };
        let f = p.fingerprint(); // 形如 "2729 CD5E E522 D9FB"

        // 抄写噪声：大小写、空格、换行、制表符、连字符
        assert!(p.fingerprint_matches(&f));
        assert!(p.fingerprint_matches(&f.to_lowercase()));
        assert!(p.fingerprint_matches(&f.replace(' ', "")));
        assert!(p.fingerprint_matches(&f.replace(' ', "-")));
        assert!(p.fingerprint_matches(&format!("  {f}\n")));
        assert!(p.fingerprint_matches(&f.replace(' ', "\t")));

        // 差一位就必须判不一致 —— 放宽一位等于把攻击者要碰撞的位数少一位
        let mut wrong: Vec<char> = f.replace(' ', "").chars().collect();
        wrong[0] = if wrong[0] == 'A' { 'B' } else { 'A' };
        assert!(!p.fingerprint_matches(&wrong.iter().collect::<String>()));

        // 截断、前缀、空串都不算一致
        assert!(!p.fingerprint_matches(&f[..9]));
        assert!(!p.fingerprint_matches(""));
        assert!(!p.fingerprint_matches("   "));
        assert!(!p.fingerprint_matches(&format!("{f}00")));

        // 别的服务器的指纹当然也不算
        let other = ServerProfile {
            base_url: "http://a".into(),
            pubkey: format!("04{}", "cd".repeat(64)),
        };
        assert!(!p.fingerprint_matches(&other.fingerprint()));
    }
}
