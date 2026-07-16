//! 对称加解密（AES / TripleDES / RC4 / Rabbit），OpenSSL 盐格式（与 crypto-js 默认兼容）。
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

/// Rabbit 流密码（RFC 4503）。crypto-js 的 `Rabbit`（非 Legacy）与 RFC 字节序一致，
/// 因此按 RFC 实现即与 crypto-js 互通。
struct Rabbit {
    x: [u32; 8],
    c: [u32; 8],
    carry: u32, // 0 或 1
}

const RABBIT_A: [u32; 8] = [
    0x4D34_D34D,
    0xD34D_34D3,
    0x34D3_4D34,
    0x4D34_D34D,
    0xD34D_34D3,
    0x34D3_4D34,
    0x4D34_D34D,
    0xD34D_34D3,
];

impl Rabbit {
    fn new(key: &[u8; 16], iv: Option<&[u8; 8]>) -> Self {
        // 16 位子密钥（RFC 4503 §2.3）：密钥按打印序视作大端 128 位数，
        // K0 取最低 16 位，即末两个字节。
        let mut k = [0u16; 8];
        for (i, w) in k.iter_mut().enumerate() {
            *w = u16::from_be_bytes([key[14 - 2 * i], key[15 - 2 * i]]);
        }
        let mut x = [0u32; 8];
        let mut c = [0u32; 8];
        for j in 0..8 {
            if j % 2 == 0 {
                x[j] = ((k[(j + 1) % 8] as u32) << 16) | k[j] as u32;
                c[j] = ((k[(j + 4) % 8] as u32) << 16) | k[(j + 5) % 8] as u32;
            } else {
                x[j] = ((k[(j + 5) % 8] as u32) << 16) | k[(j + 4) % 8] as u32;
                c[j] = ((k[j] as u32) << 16) | k[(j + 1) % 8] as u32;
            }
        }
        let mut r = Self { x, c, carry: 0 };
        for _ in 0..4 {
            r.next_state();
        }
        for j in 0..8 {
            r.c[j] ^= r.x[(j + 4) % 8];
        }
        // IV 装配（RFC 4503 §2.4）
        if let Some(iv) = iv {
            let i0 = u32::from_be_bytes([iv[4], iv[5], iv[6], iv[7]]);
            let i2 = u32::from_be_bytes([iv[0], iv[1], iv[2], iv[3]]);
            let i1 = (i2 & 0xFFFF_0000) | (i0 >> 16);
            let i3 = (i2 << 16) | (i0 & 0x0000_FFFF);
            r.c[0] ^= i0;
            r.c[1] ^= i1;
            r.c[2] ^= i2;
            r.c[3] ^= i3;
            r.c[4] ^= i0;
            r.c[5] ^= i1;
            r.c[6] ^= i2;
            r.c[7] ^= i3;
            for _ in 0..4 {
                r.next_state();
            }
        }
        r
    }

    /// 计数器 + g 函数迭代一轮（RFC 4503 §2.5）。
    fn next_state(&mut self) {
        for (c, a) in self.c.iter_mut().zip(RABBIT_A) {
            let t = *c as u64 + a as u64 + self.carry as u64;
            self.carry = (t >> 32) as u32;
            *c = t as u32;
        }
        let mut g = [0u32; 8];
        for (g, (&x, &c)) in g.iter_mut().zip(self.x.iter().zip(self.c.iter())) {
            let u = x.wrapping_add(c) as u64;
            let sq = u * u;
            *g = (sq ^ (sq >> 32)) as u32;
        }
        let x = &mut self.x;
        x[0] = g[0]
            .wrapping_add(g[7].rotate_left(16))
            .wrapping_add(g[6].rotate_left(16));
        x[1] = g[1].wrapping_add(g[0].rotate_left(8)).wrapping_add(g[7]);
        x[2] = g[2]
            .wrapping_add(g[1].rotate_left(16))
            .wrapping_add(g[0].rotate_left(16));
        x[3] = g[3].wrapping_add(g[2].rotate_left(8)).wrapping_add(g[1]);
        x[4] = g[4]
            .wrapping_add(g[3].rotate_left(16))
            .wrapping_add(g[2].rotate_left(16));
        x[5] = g[5].wrapping_add(g[4].rotate_left(8)).wrapping_add(g[3]);
        x[6] = g[6]
            .wrapping_add(g[5].rotate_left(16))
            .wrapping_add(g[4].rotate_left(16));
        x[7] = g[7].wrapping_add(g[6].rotate_left(8)).wrapping_add(g[5]);
    }

    /// 产出下一个 128 位密钥流块（RFC 4503 §2.6，小端输出）。
    fn block(&mut self) -> [u8; 16] {
        self.next_state();
        let x = &self.x;
        let s: [u16; 8] = [
            (x[0] as u16) ^ ((x[5] >> 16) as u16),
            ((x[0] >> 16) as u16) ^ (x[3] as u16),
            (x[2] as u16) ^ ((x[7] >> 16) as u16),
            ((x[2] >> 16) as u16) ^ (x[5] as u16),
            (x[4] as u16) ^ ((x[1] >> 16) as u16),
            ((x[4] >> 16) as u16) ^ (x[7] as u16),
            (x[6] as u16) ^ ((x[3] >> 16) as u16),
            ((x[6] >> 16) as u16) ^ (x[1] as u16),
        ];
        // 输出同样按大端呈现：S[127:112] 在前，每个 16 位字大端。
        let mut out = [0u8; 16];
        for (i, w) in s.iter().rev().enumerate() {
            out[2 * i..2 * i + 2].copy_from_slice(&w.to_be_bytes());
        }
        out
    }
}

/// 用 Rabbit 密钥流异或数据（加解密同一操作）。
fn rabbit_xor(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut k = [0u8; 16];
    k.copy_from_slice(&key[..16]);
    let mut i = [0u8; 8];
    i.copy_from_slice(&iv[..8]);
    let mut r = Rabbit::new(&k, Some(&i));
    let mut block = [0u8; 16];
    let mut out = Vec::with_capacity(data.len());
    for (n, &b) in data.iter().enumerate() {
        if n % 16 == 0 {
            block = r.block();
        }
        out.push(b ^ block[n % 16]);
    }
    out
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len().max(1)]);
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
        Algo::Rabbit => rabbit_xor(&k, &iv, data),
    };

    let mut out = b"Salted__".to_vec();
    out.extend_from_slice(&salt);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

/// 解密 OpenSSL 盐格式的 base64 密文。
pub fn decrypt(algo: Algo, b64: &str, password: &str) -> Result<String, String> {
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
        Algo::Rabbit => rabbit_xor(&k, &iv, ct),
    };
    String::from_utf8(pt).map_err(|_| "解密结果不是有效 UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all() {
        for algo in Algo::ALL {
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

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    /// RFC 4503 §6.1：无 IV 装配的三组官方向量。
    #[test]
    fn rabbit_rfc4503_no_iv() {
        let cases = [
            (
                "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
                "B1 57 54 F0 36 A5 D6 EC F5 6B 45 26 1C 4A F7 02 \
                 88 E8 D8 15 C5 9C 0C 39 7B 69 6C 47 89 C6 8A A7 \
                 F4 16 A1 C3 70 0C D4 51 DA 68 D1 88 16 73 D6 96",
            ),
            (
                "91 28 13 29 2E 3D 36 FE 3B FC 62 F1 DC 51 C3 AC",
                "3D 2D F3 C8 3E F6 27 A1 E9 7F C3 84 87 E2 51 9C \
                 F5 76 CD 61 F4 40 5B 88 96 BF 53 AA 85 54 FC 19 \
                 E5 54 74 73 FB DB 43 50 8A E5 3B 20 20 4D 4C 5E",
            ),
            (
                "83 95 74 15 87 E0 C7 33 E9 E9 AB 01 C0 9B 00 43",
                "0C B1 0D CD A0 41 CD AC 32 EB 5C FD 02 D0 60 9B \
                 95 FC 9F CA 0F 17 01 5A 7B 70 92 11 4C FF 3E AD \
                 96 49 E5 DE 8B FC 7F 3F 92 41 47 AD 3A 94 74 28",
            ),
        ];
        for (key_hex, ks_hex) in cases {
            let key: [u8; 16] = hex(key_hex).try_into().unwrap();
            let mut r = Rabbit::new(&key, None);
            let ks: Vec<u8> = (0..3).flat_map(|_| r.block()).collect();
            assert_eq!(ks, hex(ks_hex));
        }
    }

    /// RFC 4503 §6.2：零密钥 + 三组 IV 的官方向量。
    #[test]
    fn rabbit_rfc4503_with_iv() {
        let key = [0u8; 16];
        let cases = [
            (
                "00 00 00 00 00 00 00 00",
                "C6 A7 27 5E F8 54 95 D8 7C CD 5D 37 67 05 B7 ED \
                 5F 29 A6 AC 04 F5 EF D4 7B 8F 29 32 70 DC 4A 8D \
                 2A DE 82 2B 29 DE 6C 1E E5 2B DB 8A 47 BF 8F 66",
            ),
            (
                "C3 73 F5 75 C1 26 7E 59",
                "1F CD 4E B9 58 00 12 E2 E0 DC CC 92 22 01 7D 6D \
                 A7 5F 4E 10 D1 21 25 01 7B 24 99 FF ED 93 6F 2E \
                 EB C1 12 C3 93 E7 38 39 23 56 BD D0 12 02 9B A7",
            ),
            (
                "A6 EB 56 1A D2 F4 17 27",
                "44 5A D8 C8 05 85 8D BF 70 B6 AF 23 A1 51 10 4D \
                 96 C8 F2 79 47 F4 2C 5B AE AE 67 C6 AC C3 5B 03 \
                 9F CB FC 89 5F A7 1C 17 31 3D F0 34 F0 15 51 CB",
            ),
        ];
        for (iv_hex, ks_hex) in cases {
            let iv: [u8; 8] = hex(iv_hex).try_into().unwrap();
            let mut r = Rabbit::new(&key, Some(&iv));
            let ks: Vec<u8> = (0..3).flat_map(|_| r.block()).collect();
            assert_eq!(ks, hex(ks_hex));
        }
    }
}
