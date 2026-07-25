//! 发布签名校验 —— 自动更新的**最后一道也是最重要的一道**锁。
//!
//! # 为什么光有传输加密不够
//!
//! 传输公钥固定保证了「对面是我指定的服务器」，sha256 保证了「字节没在路上被换」。
//! 但两者都**不保证服务器上那个文件是发布者放的** —— 服务器一旦被拿下（管理员 JWT 走
//! 明文头、后台 SPA 也走明文 HTTP，都是现实可行的入口），攻击者就能上传并发布恶意安装包，
//! 而客户端的公钥固定、GCM tag、sha256 会**全部正常通过**。
//!
//! 发布签名用的是一把**永不上线**的密钥（服务端仓库的 `examples/release-sign.rs`），
//! 私钥只在发布者手里。于是「服务器沦陷」的后果从「全网 RCE」降级为「拒绝服务」。
//!
//! # 待签清单必须与服务端逐字节一致
//!
//! ⚠️ 服务端 `ferric-server` 仓库的 `server/src/release.rs` 里有一份**等价实现**。
//! 两个仓库各自独立（跨仓 path 依赖会让单独 clone 任一仓库都构建不了），所以靠
//! **同一组固定测试向量**在两边各断言一次来防漂移 —— 见本文件与服务端同名的
//! `signing_payload_is_byte_exact`。改这里必须同步改服务端，两边向量测试都要过。

use libsm::sm2::signature::{SigCtx, Signature};

/// 清单格式版本。要改字段集合就**升这个前缀**，别就地改格式 ——
/// 否则新旧两端会各自算出不同的字节而谁也验不过谁。
pub const PAYLOAD_TAG: &str = "ferric-update-v1";

/// 组装待签字节。字段顺序、分隔符、行尾换行全部固定，无任何空白容错。
pub fn signing_payload(
    version: &str,
    build: i64,
    platform: &str,
    arch: &str,
    sha256: &str,
    size: i64,
) -> Vec<u8> {
    format!(
        "{PAYLOAD_TAG}\nversion={version}\nbuild={build}\nplatform={platform}\narch={arch}\nsha256={sha256}\nsize={size}\n"
    )
    .into_bytes()
}

/// 编译期烘入的发布验签公钥（见 `build.rs`）。未配置则返回 None，
/// 此时更新流程会拒绝执行任何安装包。
pub fn builtin_pubkey() -> Option<&'static str> {
    let k = env!("FERRIC_RELEASE_PUBKEY").trim();
    (!k.is_empty()).then_some(k)
}

/// 校验签名。所有长度/编码守卫都在调 libsm **之前**完成 ——
/// 签名与公钥都来自网络/配置，是外部输入，不能直接喂进去。
pub fn verify(pubkey_hex: &str, sig_hex: &str, payload: &[u8]) -> Result<(), String> {
    let pk_bytes = hex::decode(pubkey_hex.trim()).map_err(|_| "验签公钥不是合法 hex".to_owned())?;
    if pk_bytes.len() != 65 || pk_bytes[0] != 0x04 {
        return Err("验签公钥须为未压缩格式（04 开头，65 字节）".into());
    }
    let sig_bytes = hex::decode(sig_hex.trim()).map_err(|_| "签名不是合法 hex".to_owned())?;
    if sig_bytes.is_empty() || sig_bytes.len() > 256 {
        return Err("签名长度非法".into());
    }

    let ctx = SigCtx::new();
    let pk = ctx
        .load_pubkey(&pk_bytes)
        .map_err(|_| "验签公钥非法".to_owned())?;
    let sig = Signature::der_decode(&sig_bytes).map_err(|_| "签名编码非法".to_owned())?;
    match ctx.verify(payload, &pk, &sig) {
        Ok(true) => Ok(()),
        _ => Err("发布签名校验不通过 —— 该安装包不是由发布者签发的，已拒绝执行".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跨仓库的锚：服务端 `server/src/release.rs` 有一份同名测试断言**完全相同**的字节。
    /// 这个测试挂了就说明清单格式被改动，两端必须同步，否则线上会全线验签失败。
    #[test]
    fn signing_payload_is_byte_exact() {
        let p = signing_payload("0.6.0", 60, "windows", "x86_64", "abc123", 1024);
        assert_eq!(
            String::from_utf8(p).unwrap(),
            "ferric-update-v1\nversion=0.6.0\nbuild=60\nplatform=windows\narch=x86_64\nsha256=abc123\nsize=1024\n"
        );
    }

    #[test]
    fn sign_verify_roundtrip_and_rejects_swapped_manifest() {
        let ctx = SigCtx::new();
        let (pk, sk) = ctx.new_keypair().unwrap();
        let pk_hex = hex::encode(ctx.serialize_pubkey(&pk, false).unwrap());
        let payload = signing_payload("0.6.0", 60, "linux", "x86_64", "deadbeef", 42);
        let sig = hex::encode(ctx.sign(&payload, &sk, &pk).unwrap().der_encode());

        assert!(verify(&pk_hex, &sig, &payload).is_ok());

        // 拿这份签名去冒充另一个版本 —— 必须失败
        let other = signing_payload("9.9.9", 999, "linux", "x86_64", "deadbeef", 42);
        assert!(verify(&pk_hex, &sig, &other).is_err());

        // 同版本但换了文件（sha256 变了）—— 必须失败
        let tampered = signing_payload("0.6.0", 60, "linux", "x86_64", "cafebabe", 42);
        assert!(verify(&pk_hex, &sig, &tampered).is_err());

        // 换一把公钥 —— 必须失败
        let (pk2, _) = ctx.new_keypair().unwrap();
        let pk2_hex = hex::encode(ctx.serialize_pubkey(&pk2, false).unwrap());
        assert!(verify(&pk2_hex, &sig, &payload).is_err());
    }

    /// 畸形输入一律返回错误，绝不能 panic —— 这些值来自网络。
    #[test]
    fn malformed_input_never_panics() {
        let payload = signing_payload("1.0.0", 1, "linux", "x86_64", "aa", 1);
        for (pk, sig) in [
            ("", ""),
            ("zz", "zz"),
            ("04", "aabb"),
            (&"04".repeat(65), ""),
            (&"aa".repeat(65), "aabbcc"),
            (&format!("04{}", "00".repeat(64)), "3045"),
        ] {
            assert!(verify(pk, sig, &payload).is_err());
        }
    }
}
