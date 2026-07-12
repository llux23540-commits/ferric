//! RSA 密钥对生成（PEM）。

use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
use rsa::pkcs8::EncodePublicKey;
use rsa::{RsaPrivateKey, RsaPublicKey};

/// 生成 RSA 密钥对，返回 `(公钥 PEM, 私钥 PEM)`。
///
/// 公钥为 SPKI（`BEGIN PUBLIC KEY`），私钥为 PKCS#1（`BEGIN RSA PRIVATE KEY`）。
/// 位数会被夹到 256..=4096；耗时较长，调用方应放到后台线程。
pub fn generate(bits: usize) -> Result<(String, String), String> {
    let bits = bits.clamp(256, 4096);
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, bits).map_err(|e| e.to_string())?;
    let pub_key = RsaPublicKey::from(&priv_key);

    let priv_pem = priv_key
        .to_pkcs1_pem(LineEnding::LF)
        .map_err(|e| e.to_string())?
        .to_string();
    let pub_pem = pub_key
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| e.to_string())?;
    Ok((pub_pem, priv_pem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_pem_headers() {
        let (pub_pem, priv_pem) = generate(512).unwrap();
        assert!(pub_pem.starts_with("-----BEGIN PUBLIC KEY-----"));
        assert!(priv_pem.starts_with("-----BEGIN RSA PRIVATE KEY-----"));
    }
}
