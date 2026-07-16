//! 国密：SM2（非对称）/ SM3（摘要）/ SM4（对称，ECB/CBC/CTR/CFB/OFB 全模式）。
//!
//! ECB/CBC 走 smcrypto；CTR/CFB/OFB 走 RustCrypto 的 `sm4` + 流模式组合。
//! 除 ECB 外均为「随机 IV(16B) 拼在密文前」的格式，每次加密结果都不同。

use cipher::{AsyncStreamCipher, KeyIvInit, StreamCipher};
use serde::{Deserialize, Serialize};
use smcrypto::{sm2, sm3, sm4};

type Sm4Ctr = ctr::Ctr128BE<::sm4::Sm4>;
type Sm4Ofb = ofb::Ofb<::sm4::Sm4>;
type Sm4CfbEnc = cfb_mode::Encryptor<::sm4::Sm4>;
type Sm4CfbDec = cfb_mode::Decryptor<::sm4::Sm4>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncAlgo {
    /// SM4 ECB：确定性——同明文同口令结果固定。旧版草稿存的 "Sm4" 即此模式。
    #[serde(alias = "Sm4")]
    Sm4Ecb,
    /// SM4 CBC：每次随机 IV，同明文同口令每次结果都不同。
    Sm4Cbc,
    /// SM4 CTR：计数器流模式，随机 nonce，密文与明文等长。
    Sm4Ctr,
    /// SM4 CFB：密文反馈流模式，随机 IV。
    Sm4Cfb,
    /// SM4 OFB：输出反馈流模式，随机 IV。
    Sm4Ofb,
    Sm2,
    Sm3,
}

impl EncAlgo {
    pub const ALL: [EncAlgo; 7] = [
        EncAlgo::Sm4Ecb,
        EncAlgo::Sm4Cbc,
        EncAlgo::Sm4Ctr,
        EncAlgo::Sm4Cfb,
        EncAlgo::Sm4Ofb,
        EncAlgo::Sm2,
        EncAlgo::Sm3,
    ];
    pub fn label(self) -> &'static str {
        match self {
            EncAlgo::Sm4Ecb => "SM4-ECB · 对称（结果固定）",
            EncAlgo::Sm4Cbc => "SM4-CBC · 对称（随机 IV · 每次不同）",
            EncAlgo::Sm4Ctr => "SM4-CTR · 对称（随机 nonce · 每次不同）",
            EncAlgo::Sm4Cfb => "SM4-CFB · 对称（随机 IV · 每次不同）",
            EncAlgo::Sm4Ofb => "SM4-OFB · 对称（随机 IV · 每次不同）",
            EncAlgo::Sm2 => "SM2 · 公钥加密",
            EncAlgo::Sm3 => "SM3 · 摘要",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecAlgo {
    #[serde(alias = "Sm4")]
    Sm4Ecb,
    Sm4Cbc,
    Sm4Ctr,
    Sm4Cfb,
    Sm4Ofb,
    Sm2,
}

impl DecAlgo {
    pub const ALL: [DecAlgo; 6] = [
        DecAlgo::Sm4Ecb,
        DecAlgo::Sm4Cbc,
        DecAlgo::Sm4Ctr,
        DecAlgo::Sm4Cfb,
        DecAlgo::Sm4Ofb,
        DecAlgo::Sm2,
    ];
    pub fn label(self) -> &'static str {
        match self {
            DecAlgo::Sm4Ecb => "SM4-ECB · 对称解密",
            DecAlgo::Sm4Cbc => "SM4-CBC · 对称解密（IV+密文）",
            DecAlgo::Sm4Ctr => "SM4-CTR · 对称解密（IV+密文）",
            DecAlgo::Sm4Cfb => "SM4-CFB · 对称解密（IV+密文）",
            DecAlgo::Sm4Ofb => "SM4-OFB · 对称解密（IV+密文）",
            DecAlgo::Sm2 => "SM2 · 私钥解密",
        }
    }
}

/// 生成 SM2 密钥对，返回 `(公钥 04+128hex, 私钥 64hex)`。
pub fn gen_sm2_keypair() -> (String, String) {
    let (priv_k, pub_k) = sm2::gen_keypair();
    (format!("04{pub_k}"), priv_k)
}

/// SM2 密文格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sm2Fmt {
    C1C3C2,
    C1C2C3,
    Asn1,
}

impl Sm2Fmt {
    pub const ALL: [Sm2Fmt; 3] = [Sm2Fmt::C1C3C2, Sm2Fmt::C1C2C3, Sm2Fmt::Asn1];
    pub fn label(self) -> &'static str {
        match self {
            Sm2Fmt::C1C3C2 => "C1C3C2 · 新国标（默认）",
            Sm2Fmt::C1C2C3 => "C1C2C3 · 旧国标",
            Sm2Fmt::Asn1 => "ASN.1 / DER",
        }
    }
}

fn valid_sm2_pk(key: &str) -> Result<String, String> {
    let pk = key.trim().to_string();
    if !sm2::pubkey_valid(&pk) {
        return Err("SM2 公钥无效（应为 04 开头 130 hex）".into());
    }
    Ok(pk)
}

fn valid_sm2_sk(key: &str) -> Result<String, String> {
    let sk = key.trim().to_string();
    if !sm2::privkey_valid(&sk) {
        return Err("SM2 私钥无效（应为 64 hex）".into());
    }
    Ok(sk)
}

/// SM2 按指定密文格式加密，输出 hex。
pub fn sm2_encrypt_fmt(fmt: Sm2Fmt, text: &str, pubkey: &str) -> Result<String, String> {
    let pk = valid_sm2_pk(pubkey)?;
    let data = text.as_bytes().to_vec();
    guard(move || {
        let enc = sm2::Encrypt::new(&pk);
        match fmt {
            Sm2Fmt::C1C3C2 => enc.encrypt_hex(&data),
            Sm2Fmt::C1C2C3 => to_hex(&enc.encrypt_c1c2c3(&data)),
            Sm2Fmt::Asn1 => to_hex(&enc.encrypt_asna1(&data)),
        }
    })
}

/// SM2 按指定密文格式解密 hex 密文。
pub fn sm2_decrypt_fmt(fmt: Sm2Fmt, hex_str: &str, privkey: &str) -> Result<String, String> {
    let sk = valid_sm2_sk(privkey)?;
    let hex_str = hex_str.trim().to_string();
    let pt = match fmt {
        Sm2Fmt::C1C3C2 => guard(move || sm2::Decrypt::new(&sk).decrypt_hex(&hex_str))?,
        Sm2Fmt::C1C2C3 => {
            let raw = from_hex(&hex_str)?;
            guard(move || sm2::Decrypt::new(&sk).decrypt_c1c2c3(&raw))?
        }
        Sm2Fmt::Asn1 => {
            let raw = from_hex(&hex_str)?;
            guard(move || sm2::Decrypt::new(&sk).decrypt_asna1(&raw))?
        }
    };
    String::from_utf8(pt).map_err(|_| "解密结果不是有效 UTF-8".into())
}

/// SM2 签名（SM3 摘要 + 默认 ID `1234567812345678`），输出 ASN.1/DER 签名的 hex。
pub fn sm2_sign(text: &str, privkey: &str) -> Result<String, String> {
    let sk = valid_sm2_sk(privkey)?;
    let data = text.as_bytes().to_vec();
    let sig = guard(move || sm2::Sign::new(&sk).sign(&data))?;
    if sig.is_empty() {
        return Err("签名失败，请重试".into());
    }
    Ok(to_hex(&sig))
}

/// SM2 验签（与 [`sm2_sign`] 对应的 DER hex 签名）。
pub fn sm2_verify(text: &str, sig_hex: &str, pubkey: &str) -> Result<bool, String> {
    let pk = valid_sm2_pk(pubkey)?;
    let sig = from_hex(sig_hex)?;
    if sig.is_empty() {
        return Err("请输入签名值（hex）".into());
    }
    let data = text.as_bytes().to_vec();
    guard(move || sm2::Verify::new(&pk).verify(&data, &sig))
}

/// 由私钥反推公钥（04 前缀 130 hex）。
pub fn sm2_pk_from_sk(privkey: &str) -> Result<String, String> {
    let sk = valid_sm2_sk(privkey)?;
    guard(move || format!("04{}", sm2::pk_from_sk(&sk)))
}

/// 密钥对导出为 PEM 文件（含私钥）。
pub fn sm2_keypair_to_pem(privkey: &str, path: &str) -> Result<(), String> {
    let sk = valid_sm2_sk(privkey)?;
    let path = path.to_string();
    guard(move || sm2::keypair_to_pem_file(&sk, &path))
}

/// 从 PEM 文件导入密钥对，返回 `(公钥 04+128hex, 私钥 64hex)`。
pub fn sm2_keypair_from_pem(path: &str) -> Result<(String, String), String> {
    let path = path.to_string();
    let (sk, pk) = guard(move || sm2::keypair_from_pem_file(&path))?;
    if sk.is_empty() || !sm2::privkey_valid(&sk) {
        return Err("PEM 解析失败或不是 SM2 私钥文件".into());
    }
    let pk_full = if pk.len() == 130 {
        pk
    } else {
        format!("04{pk}")
    };
    Ok((pk_full, sk))
}

fn guard<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
    std::panic::catch_unwind(f).map_err(|_| "运算失败：输入或密钥不合法".to_string())
}

fn sm4_key(pass: &str) -> [u8; 16] {
    // 口令经 SM3 派生 16 字节密钥。
    sm3::sm3_hash_raw(pass.as_bytes())[..16]
        .try_into()
        .expect("SM3 输出至少 16 字节")
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if !s.is_ascii() || s.len() % 2 != 0 {
        return Err("无效 hex 密文".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "无效 hex 密文".to_string()))
        .collect()
}

/// 拆分「IV(16B) + 密文」格式的 hex 输入。流模式密文可为空（空明文）。
fn split_iv_ct(hex: &str) -> Result<([u8; 16], Vec<u8>), String> {
    let raw = from_hex(hex)?;
    if raw.len() < 16 {
        return Err("密文过短：应为 IV(16 字节) + 密文".into());
    }
    let (iv, ct) = raw.split_at(16);
    Ok((iv.try_into().expect("IV 长度固定 16"), ct.to_vec()))
}

/// 流模式（CTR/CFB/OFB）加密：随机 IV，输出 `hex(IV) + hex(密文)`。
fn sm4_stream_encrypt(algo: EncAlgo, key16: [u8; 16], text: &str) -> String {
    let iv: [u8; 16] = rand::random();
    let mut buf = text.as_bytes().to_vec();
    match algo {
        EncAlgo::Sm4Ctr => Sm4Ctr::new(&key16.into(), &iv.into()).apply_keystream(&mut buf),
        EncAlgo::Sm4Ofb => Sm4Ofb::new(&key16.into(), &iv.into()).apply_keystream(&mut buf),
        EncAlgo::Sm4Cfb => Sm4CfbEnc::new(&key16.into(), &iv.into()).encrypt(&mut buf),
        _ => unreachable!("仅流模式调用"),
    }
    format!("{}{}", to_hex(&iv), to_hex(&buf))
}

/// 流模式（CTR/CFB/OFB）解密：输入 `hex(IV) + hex(密文)`。
fn sm4_stream_decrypt(algo: DecAlgo, key16: [u8; 16], hex: &str) -> Result<Vec<u8>, String> {
    let (iv, mut buf) = split_iv_ct(hex)?;
    match algo {
        DecAlgo::Sm4Ctr => Sm4Ctr::new(&key16.into(), &iv.into()).apply_keystream(&mut buf),
        DecAlgo::Sm4Ofb => Sm4Ofb::new(&key16.into(), &iv.into()).apply_keystream(&mut buf),
        DecAlgo::Sm4Cfb => Sm4CfbDec::new(&key16.into(), &iv.into()).decrypt(&mut buf),
        _ => unreachable!("仅流模式调用"),
    }
    Ok(buf)
}

/// 加密 / 摘要，输出 hex。
pub fn encrypt(algo: EncAlgo, text: &str, key: &str) -> Result<String, String> {
    match algo {
        EncAlgo::Sm3 => Ok(sm3::sm3_hash(text.as_bytes())),
        EncAlgo::Sm4Ecb => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            let k = sm4_key(key);
            guard(move || sm4::CryptSM4ECB::new(&k).encrypt_ecb_hex(text.as_bytes()))
        }
        EncAlgo::Sm4Cbc => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            let k = sm4_key(key);
            // 每次随机 IV → 同明文同口令每次密文都不同；IV 拼在密文前随之分发。
            let iv: [u8; 16] = rand::random();
            let ct = guard(move || sm4::CryptSM4CBC::new(&k, &iv).encrypt_cbc(text.as_bytes()))?;
            Ok(format!("{}{}", to_hex(&iv), to_hex(&ct)))
        }
        EncAlgo::Sm4Ctr | EncAlgo::Sm4Cfb | EncAlgo::Sm4Ofb => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            Ok(sm4_stream_encrypt(algo, sm4_key(key), text))
        }
        EncAlgo::Sm2 => {
            // pubkey_valid / Encrypt::new 都同时接受 130（04 前缀）与 128 hex，
            // 不要自行剥前缀——trim_start_matches 会把坐标本身的前导 04 一并剥掉。
            let pk = key.trim().to_string();
            if !sm2::pubkey_valid(&pk) {
                return Err("SM2 公钥无效（应为 04 开头 130 hex）".into());
            }
            guard(move || sm2::Encrypt::new(&pk).encrypt_hex(text.as_bytes()))
        }
    }
}

/// 解密 hex 密文，输出明文。
pub fn decrypt(algo: DecAlgo, hex: &str, key: &str) -> Result<String, String> {
    let hex = hex.trim().to_string();
    match algo {
        DecAlgo::Sm4Ecb => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            let k = sm4_key(key);
            let pt = guard(move || sm4::CryptSM4ECB::new(&k).decrypt_ecb_hex(&hex))?;
            String::from_utf8(pt).map_err(|_| "解密结果不是有效 UTF-8".into())
        }
        DecAlgo::Sm4Cbc => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            let k = sm4_key(key);
            let raw = from_hex(&hex)?;
            if raw.len() < 32 {
                return Err("SM4-CBC 密文过短：应为 IV(16 字节) + 密文".into());
            }
            let (iv, ct) = raw.split_at(16);
            let (iv, ct) = (iv.to_vec(), ct.to_vec());
            let pt = guard(move || sm4::CryptSM4CBC::new(&k, &iv).decrypt_cbc(&ct))?;
            String::from_utf8(pt).map_err(|_| "解密结果不是有效 UTF-8".into())
        }
        DecAlgo::Sm4Ctr | DecAlgo::Sm4Cfb | DecAlgo::Sm4Ofb => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            let pt = sm4_stream_decrypt(algo, sm4_key(key), &hex)?;
            String::from_utf8(pt).map_err(|_| "解密结果不是有效 UTF-8".into())
        }
        DecAlgo::Sm2 => {
            let sk = key.trim().to_string();
            if !sm2::privkey_valid(&sk) {
                return Err("SM2 私钥无效（应为 64 hex）".into());
            }
            let pt = guard(move || sm2::Decrypt::new(&sk).decrypt_hex(&hex))?;
            String::from_utf8(pt).map_err(|_| "解密结果不是有效 UTF-8".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sm3_digest_stable() {
        let a = encrypt(EncAlgo::Sm3, "abc", "").unwrap();
        assert_eq!(a.len(), 64); // 256-bit hex
        assert_eq!(a, encrypt(EncAlgo::Sm3, "abc", "").unwrap());
    }

    #[test]
    fn sm4_ecb_roundtrip() {
        let ct = encrypt(EncAlgo::Sm4Ecb, "国密 SM4", "pass").unwrap();
        let pt = decrypt(DecAlgo::Sm4Ecb, &ct, "pass").unwrap();
        assert_eq!(pt, "国密 SM4");
        // ECB 是确定性的：两次加密结果相同。
        assert_eq!(ct, encrypt(EncAlgo::Sm4Ecb, "国密 SM4", "pass").unwrap());
    }

    #[test]
    fn sm4_cbc_roundtrip_and_randomized() {
        let a = encrypt(EncAlgo::Sm4Cbc, "国密 SM4", "pass").unwrap();
        let b = encrypt(EncAlgo::Sm4Cbc, "国密 SM4", "pass").unwrap();
        // 随机 IV：同明文同口令两次密文不同。
        assert_ne!(a, b);
        // 两份密文都能正确解密。
        assert_eq!(decrypt(DecAlgo::Sm4Cbc, &a, "pass").unwrap(), "国密 SM4");
        assert_eq!(decrypt(DecAlgo::Sm4Cbc, &b, "pass").unwrap(), "国密 SM4");
    }

    #[test]
    fn sm4_cbc_rejects_short_or_bad_hex() {
        assert!(decrypt(DecAlgo::Sm4Cbc, "abcd", "pass").is_err());
        assert!(decrypt(DecAlgo::Sm4Cbc, "zz".repeat(24).as_str(), "pass").is_err());
    }

    #[test]
    fn sm4_stream_modes_roundtrip_and_randomized() {
        let cases = [
            (EncAlgo::Sm4Ctr, DecAlgo::Sm4Ctr),
            (EncAlgo::Sm4Cfb, DecAlgo::Sm4Cfb),
            (EncAlgo::Sm4Ofb, DecAlgo::Sm4Ofb),
        ];
        for (ea, da) in cases {
            let a = encrypt(ea, "国密 SM4 流模式", "pass").unwrap();
            let b = encrypt(ea, "国密 SM4 流模式", "pass").unwrap();
            // 随机 IV：两次密文不同，但都能解密。
            assert_ne!(a, b, "{ea:?} 两次加密结果应不同");
            assert_eq!(decrypt(da, &a, "pass").unwrap(), "国密 SM4 流模式");
            assert_eq!(decrypt(da, &b, "pass").unwrap(), "国密 SM4 流模式");
            // 口令错误不应解出原文。
            assert_ne!(
                decrypt(da, &a, "wrong").unwrap_or_default(),
                "国密 SM4 流模式"
            );
        }
    }

    /// 流模式密文与明文等长（IV 32 hex + 明文字节数 ×2）。
    #[test]
    fn sm4_stream_ct_length() {
        let ct = encrypt(EncAlgo::Sm4Ctr, "abc", "pass").unwrap();
        assert_eq!(ct.len(), 32 + 6);
    }

    #[test]
    fn sm2_roundtrip() {
        let (pk, sk) = gen_sm2_keypair();
        let ct = encrypt(EncAlgo::Sm2, "国密 SM2", &pk).unwrap();
        let pt = decrypt(DecAlgo::Sm2, &ct, &sk).unwrap();
        assert_eq!(pt, "国密 SM2");
    }

    /// 128 hex（无 04 前缀）的公钥形式同样可用。
    #[test]
    fn sm2_accepts_bare_128_pubkey() {
        let (pk, sk) = gen_sm2_keypair();
        let ct = encrypt(EncAlgo::Sm2, "bare", &pk[2..]).unwrap();
        assert_eq!(decrypt(DecAlgo::Sm2, &ct, &sk).unwrap(), "bare");
    }

    /// 长度恰好 128 但内容非法的公钥必须被拒绝（曾有校验逃逸）。
    #[test]
    fn sm2_rejects_invalid_128_pubkey() {
        let bad = format!("zz{}", "1".repeat(126));
        assert!(encrypt(EncAlgo::Sm2, "x", &bad).is_err());
    }

    /// 三种 SM2 密文格式都能各自往返。
    #[test]
    fn sm2_fmt_roundtrips() {
        let (pk, sk) = gen_sm2_keypair();
        for fmt in Sm2Fmt::ALL {
            let ct = sm2_encrypt_fmt(fmt, "格式互通", &pk).unwrap();
            assert_eq!(
                sm2_decrypt_fmt(fmt, &ct, &sk).unwrap(),
                "格式互通",
                "{fmt:?}"
            );
        }
    }

    #[test]
    fn sm2_sign_verify_roundtrip() {
        let (pk, sk) = gen_sm2_keypair();
        let sig = sm2_sign("待签名数据", &sk).unwrap();
        assert!(sm2_verify("待签名数据", &sig, &pk).unwrap());
        // 篡改原文 → 验签失败
        assert!(!sm2_verify("被篡改数据", &sig, &pk).unwrap());
    }

    #[test]
    fn sm2_pk_derives_from_sk() {
        let (pk, sk) = gen_sm2_keypair();
        assert_eq!(sm2_pk_from_sk(&sk).unwrap(), pk);
    }

    #[test]
    fn sm2_pem_roundtrip() {
        let (pk, sk) = gen_sm2_keypair();
        let path = std::env::temp_dir().join("ferric-test-sm2.pem");
        let path = path.to_string_lossy().into_owned();
        sm2_keypair_to_pem(&sk, &path).unwrap();
        let (pk2, sk2) = sm2_keypair_from_pem(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(sk2, sk);
        assert_eq!(pk2, pk);
    }
}
