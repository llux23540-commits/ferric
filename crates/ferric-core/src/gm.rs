//! 国密：SM2（非对称）/ SM3（摘要）/ SM4（对称）。

use serde::{Deserialize, Serialize};
use smcrypto::{sm2, sm3, sm4};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncAlgo {
    /// SM4 ECB：确定性——同明文同口令结果固定。旧版草稿存的 "Sm4" 即此模式。
    #[serde(alias = "Sm4")]
    Sm4Ecb,
    /// SM4 CBC：每次随机 IV，同明文同口令每次结果都不同。
    Sm4Cbc,
    Sm2,
    Sm3,
}

impl EncAlgo {
    pub const ALL: [EncAlgo; 4] = [EncAlgo::Sm4Ecb, EncAlgo::Sm4Cbc, EncAlgo::Sm2, EncAlgo::Sm3];
    pub fn label(self) -> &'static str {
        match self {
            EncAlgo::Sm4Ecb => "SM4-ECB · 对称（结果固定）",
            EncAlgo::Sm4Cbc => "SM4-CBC · 对称（随机 IV · 每次不同）",
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
    Sm2,
}

impl DecAlgo {
    pub const ALL: [DecAlgo; 3] = [DecAlgo::Sm4Ecb, DecAlgo::Sm4Cbc, DecAlgo::Sm2];
    pub fn label(self) -> &'static str {
        match self {
            DecAlgo::Sm4Ecb => "SM4-ECB · 对称解密",
            DecAlgo::Sm4Cbc => "SM4-CBC · 对称解密（IV+密文）",
            DecAlgo::Sm2 => "SM2 · 私钥解密",
        }
    }
}

/// 生成 SM2 密钥对，返回 `(公钥 04+128hex, 私钥 64hex)`。
pub fn gen_sm2_keypair() -> (String, String) {
    let (priv_k, pub_k) = sm2::gen_keypair();
    (format!("04{pub_k}"), priv_k)
}

fn guard<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
    std::panic::catch_unwind(f).map_err(|_| "运算失败：输入或密钥不合法".to_string())
}

fn sm4_key(pass: &str) -> Vec<u8> {
    // 口令经 SM3 派生 16 字节密钥。
    sm3::sm3_hash_raw(pass.as_bytes())[..16].to_vec()
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
}
