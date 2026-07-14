//! 国密：SM2（非对称）/ SM3（摘要）/ SM4（对称）。

use serde::{Deserialize, Serialize};
use smcrypto::{sm2, sm3, sm4};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncAlgo {
    Sm4,
    Sm2,
    Sm3,
}

impl EncAlgo {
    pub const ALL: [EncAlgo; 3] = [EncAlgo::Sm4, EncAlgo::Sm2, EncAlgo::Sm3];
    pub fn label(self) -> &'static str {
        match self {
            EncAlgo::Sm4 => "SM4 · 对称加密",
            EncAlgo::Sm2 => "SM2 · 公钥加密",
            EncAlgo::Sm3 => "SM3 · 摘要",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecAlgo {
    Sm4,
    Sm2,
}

impl DecAlgo {
    pub const ALL: [DecAlgo; 2] = [DecAlgo::Sm4, DecAlgo::Sm2];
    pub fn label(self) -> &'static str {
        match self {
            DecAlgo::Sm4 => "SM4 · 对称解密",
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

/// 加密 / 摘要，输出 hex。
pub fn encrypt(algo: EncAlgo, text: &str, key: &str) -> Result<String, String> {
    match algo {
        EncAlgo::Sm3 => Ok(sm3::sm3_hash(text.as_bytes())),
        EncAlgo::Sm4 => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            let k = sm4_key(key);
            guard(move || sm4::CryptSM4ECB::new(&k).encrypt_ecb_hex(text.as_bytes()))
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
        DecAlgo::Sm4 => {
            if key.is_empty() {
                return Err("请输入 SM4 口令".into());
            }
            let k = sm4_key(key);
            let pt = guard(move || sm4::CryptSM4ECB::new(&k).decrypt_ecb_hex(&hex))?;
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
    fn sm4_roundtrip() {
        let ct = encrypt(EncAlgo::Sm4, "国密 SM4", "pass").unwrap();
        let pt = decrypt(DecAlgo::Sm4, &ct, "pass").unwrap();
        assert_eq!(pt, "国密 SM4");
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
