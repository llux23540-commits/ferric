//! 对称加解密（AES / TripleDES / RC4），OpenSSL 盐格式（与 crypto-js 默认兼容）。
//!
//! 输出为 base64(`"Salted__"` + 8 字节盐 + 密文)，密钥/IV 用 `EVP_BytesToKey`(MD5) 从口令派生。

use aes::Aes256;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use des::TdesEde3;
use md5::{Digest, Md5};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Algo {
    Aes,
    TripleDes,
    Rc4,
    Rabbit,
}

impl Algo {
    pub const ALL: [Algo; 4] = [Algo::Aes, Algo::TripleDes, Algo::Rabbit, Algo::Rc4];
    pub fn label(self) -> &'static str {
        match self {
            Algo::Aes => "AES",
            Algo::TripleDes => "TripleDES",
            Algo::Rabbit => "Rabbit",
            Algo::Rc4 => "RC4",
        }
    }
    fn key_iv_len(self) -> (usize, usize) {
        match self {
            Algo::Aes => (32, 16),
            Algo::TripleDes => (24, 8),
            Algo::Rc4 => (32, 0),
            Algo::Rabbit => (16, 8),
        }
    }
}

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;
type TdesCbcEnc = cbc::Encryptor<TdesEde3>;
type TdesCbcDec = cbc::Decryptor<TdesEde3>;

/// OpenSSL EVP_BytesToKey（MD5），派生 key||iv。
fn evp_bytes_to_key(pass: &[u8], salt: &[u8], key_len: usize, iv_len: usize) -> (Vec<u8>, Vec<u8>) {
    let mut derived = Vec::new();
    let mut prev = Vec::new();
    while derived.len() < key_len + iv_len {
        let mut h = Md5::new();
        h.update(&prev);
        h.update(pass);
        h.update(salt);
        prev = h.finalize().to_vec();
        derived.extend_from_slice(&prev);
    }
    (
        derived[..key_len].to_vec(),
        derived[key_len..key_len + iv_len].to_vec(),
    )
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j
            .wrapping_add(s[i])
            .wrapping_add(key[i % key.len().max(1)]);
        s.swap(i, j as usize);
    }
    let mut out = Vec::with_capacity(data.len());
    let (mut i, mut j) = (0u8, 0u8);
    for &b in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[(s[i as usize].wrapping_add(s[j as usize])) as usize];
        out.push(b ^ k);
    }
    out
}

/// 加密明文，返回 OpenSSL 盐格式的 base64。
pub fn encrypt(algo: Algo, text: &str, password: &str) -> Result<String, String> {
    if algo == Algo::Rabbit {
        return Err("Rabbit 暂未实现，请选择 AES / TripleDES / RC4".into());
    }
    if password.is_empty() {
        return Err("请输入密钥".into());
    }
    let salt: [u8; 8] = rand::random();
    let (key_len, iv_len) = algo.key_iv_len();
    let (k, iv) = evp_bytes_to_key(password.as_bytes(), &salt, key_len, iv_len);
    let data = text.as_bytes();

    let ct = match algo {
        Algo::Aes => Aes256CbcEnc::new_from_slices(&k, &iv)
            .map_err(|e| e.to_string())?
            .encrypt_padded_vec_mut::<Pkcs7>(data),
        Algo::TripleDes => TdesCbcEnc::new_from_slices(&k, &iv)
            .map_err(|e| e.to_string())?
            .encrypt_padded_vec_mut::<Pkcs7>(data),
        Algo::Rc4 => rc4(&k, data),
        Algo::Rabbit => unreachable!(),
    };

    let mut out = b"Salted__".to_vec();
    out.extend_from_slice(&salt);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

/// 解密 OpenSSL 盐格式的 base64 密文。
pub fn decrypt(algo: Algo, b64: &str, password: &str) -> Result<String, String> {
    if algo == Algo::Rabbit {
        return Err("Rabbit 暂未实现，请选择 AES / TripleDES / RC4".into());
    }
    let raw = B64
        .decode(b64.trim().as_bytes())
        .map_err(|_| "密文不是有效的 base64".to_string())?;
    if raw.len() < 16 || &raw[..8] != b"Salted__" {
        return Err("非 OpenSSL 盐格式密文（应以 Salted__ 开头）".into());
    }
    let salt = &raw[8..16];
    let ct = &raw[16..];
    let (key_len, iv_len) = algo.key_iv_len();
    let (k, iv) = evp_bytes_to_key(password.as_bytes(), salt, key_len, iv_len);

    let pt = match algo {
        Algo::Aes => Aes256CbcDec::new_from_slices(&k, &iv)
            .map_err(|e| e.to_string())?
            .decrypt_padded_vec_mut::<Pkcs7>(ct)
            .map_err(|_| "解密失败（密钥或算法不匹配）".to_string())?,
        Algo::TripleDes => TdesCbcDec::new_from_slices(&k, &iv)
            .map_err(|e| e.to_string())?
            .decrypt_padded_vec_mut::<Pkcs7>(ct)
            .map_err(|_| "解密失败（密钥或算法不匹配）".to_string())?,
        Algo::Rc4 => rc4(&k, ct),
        Algo::Rabbit => unreachable!(),
    };
    String::from_utf8(pt).map_err(|_| "解密结果不是有效 UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all() {
        for algo in [Algo::Aes, Algo::TripleDes, Algo::Rc4] {
            let ct = encrypt(algo, "hello 国密 world", "s3cret").unwrap();
            assert!(ct.starts_with("U2FsdGVk")); // "Salted__" 的 base64 前缀
            let pt = decrypt(algo, &ct, "s3cret").unwrap();
            assert_eq!(pt, "hello 国密 world");
        }
    }

    #[test]
    fn wrong_key_fails_aes() {
        let ct = encrypt(Algo::Aes, "secret", "k1").unwrap();
        assert!(decrypt(Algo::Aes, &ct, "k2").is_err());
    }
}
