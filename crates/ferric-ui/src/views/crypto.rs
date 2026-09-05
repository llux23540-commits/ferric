//! 加密 / 解密文本 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `CryptoView`：左「加密」右「解密」两栏对称，
//! 各有算法下拉、口令、输入、输出。算法与实现全在 `ferric_core::crypto`
//!（与 crypto-js 兼容的 OpenSSL 盐格式），没动过。
//!
//! 两栏刻意独立（各自的算法与口令）：常见用法是「用 A 算法加密、
//! 验证另一份 B 算法的密文」，共用一套参数会让人反复来回切。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::crypto::{self, Algo};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct CryptoDraft {
    enc_text: String,
    dec_text: String,
    enc_algo: Algo,
    dec_algo: Algo,
}

/// 一侧（加密或解密）的状态。两侧同构，收成一个结构避免字段名前缀满天飞。
pub struct Side {
    pub algo: Algo,
    pub key: String,
    pub input: TextBuffer,
    pub output: TextBuffer,
    pub ok: bool,
    pub status: String,
}

impl Side {
    fn new(algo: Algo) -> Self {
        Self {
            algo,
            key: String::new(),
            input: TextBuffer::new(""),
            output: TextBuffer::new(""),
            ok: true,
            status: "就绪".to_owned(),
        }
    }

    pub fn algo_index(&self) -> i32 {
        Algo::ALL.iter().position(|a| *a == self.algo).unwrap_or(0) as i32
    }

    fn set_algo_index(&mut self, i: i32) {
        if let Some(a) = Algo::ALL.get(i.max(0) as usize) {
            self.algo = *a;
        }
    }
}

pub struct CryptoTool {
    pub enc: Side,
    pub dec: Side,
}

impl Default for CryptoTool {
    fn default() -> Self {
        Self {
            enc: Side::new(Algo::AesCbc),
            dec: Side::new(Algo::AesCbc),
        }
    }
}

impl CryptoTool {
    /// 算法下拉的标签（两侧共用一份）。
    pub fn algo_labels() -> Vec<&'static str> {
        Algo::ALL.iter().map(|a| a.label()).collect()
    }

    pub fn encrypt(&mut self) {
        let text = self.enc.input.text();
        if text.is_empty() {
            self.enc.ok = true;
            self.enc.status = "请输入要加密的文本".to_owned();
            return;
        }
        // 空口令不拦：某些算法允许，且拦了反而挡住用户的正当用法。
        // 只在结果里如实反映成功与否。
        match crypto::encrypt(self.enc.algo, &text, &self.enc.key) {
            Ok(out) => {
                self.enc.output.set_text(&out);
                self.enc.ok = true;
                self.enc.status = format!("已加密（{}）", self.enc.algo.label());
            }
            Err(e) => {
                self.enc.ok = false;
                self.enc.status = format!("加密失败：{e}");
            }
        }
    }

    pub fn decrypt(&mut self) {
        let text = self.dec.input.text();
        if text.trim().is_empty() {
            self.dec.ok = true;
            self.dec.status = "请粘入 Base64 密文".to_owned();
            return;
        }
        match crypto::decrypt(self.dec.algo, text.trim(), &self.dec.key) {
            Ok(out) => {
                self.dec.output.set_text(&out);
                self.dec.ok = true;
                self.dec.status = format!("已解密（{}）", self.dec.algo.label());
            }
            Err(e) => {
                // 解密失败最常见的原因是口令错或算法选错。原样透出底层错误，
                // 但**不清空上一次的明文** —— 那可能是用户还要用的结果。
                self.dec.ok = false;
                self.dec.status = format!("解密失败：{e}");
            }
        }
    }

    /// 把加密结果送到解密侧（自检往返用）。连算法一起带过去 ——
    /// 只搬密文的话用户几乎一定会忘了同步算法，然后收到一条看不懂的报错。
    pub fn send_to_decrypt(&mut self) {
        let cipher = self.enc.output.text();
        if cipher.is_empty() {
            return;
        }
        self.dec.algo = self.enc.algo;
        self.dec.key = self.enc.key.clone();
        self.dec.input.set_text(&cipher);
        self.decrypt();
    }

    pub fn set_enc_algo(&mut self, i: i32) {
        self.enc.set_algo_index(i);
        self.enc.status = format!("算法：{}", self.enc.algo.label());
    }

    pub fn set_dec_algo(&mut self, i: i32) {
        self.dec.set_algo_index(i);
        self.dec.status = format!("算法：{}", self.dec.algo.label());
    }
}

impl Tool for CryptoTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "crypto",
            name: "加密 / 解密文本",
            desc: "AES（CBC/ECB/CTR/CFB/OFB）/ TripleDES / RC4 / Rabbit，OpenSSL 盐格式，与 crypto-js 兼容。",
            icon: icons::LOCK,
            group: "加密",
            keywords: &["crypto", "aes", "加密", "解密", "encrypt"],
        }
    }

    /// **只存输入与算法，不存口令**。`app.ron` 是当前用户可写的普通文件；
    /// 口令写进去等于把它交给任何同用户进程。与 egui 版一致。
    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&CryptoDraft {
            enc_text: self.enc.input.text(),
            dec_text: self.dec.input.text(),
            enc_algo: self.enc.algo,
            dec_algo: self.dec.algo,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<CryptoDraft>(data) {
            self.enc.input.set_text(&d.enc_text);
            self.dec.input.set_text(&d.dec_text);
            self.enc.algo = d.enc_algo;
            self.dec.algo = d.dec_algo;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_the_ui_state() {
        let mut t = CryptoTool::default();
        t.enc.input.set_text("hello 中文 payload");
        t.enc.key = "pw123".into();
        t.encrypt();
        assert!(t.enc.ok, "{}", t.enc.status);
        let cipher = t.enc.output.text();
        assert!(!cipher.is_empty());

        t.send_to_decrypt();
        assert!(t.dec.ok, "{}", t.dec.status);
        assert_eq!(t.dec.output.text(), "hello 中文 payload");
    }

    #[test]
    fn send_to_decrypt_carries_algo_and_key() {
        // 只搬密文的话用户几乎一定忘了同步算法，然后收到看不懂的报错。
        let mut t = CryptoTool::default();
        t.set_enc_algo(2); // AesCtr
        t.enc.input.set_text("x");
        t.enc.key = "k".into();
        t.encrypt();
        t.send_to_decrypt();
        assert_eq!(t.dec.algo, t.enc.algo);
        assert_eq!(t.dec.key, "k");
        assert!(t.dec.ok, "{}", t.dec.status);
    }

    #[test]
    fn wrong_password_reports_failure_and_keeps_previous_plaintext() {
        let mut t = CryptoTool::default();
        t.enc.input.set_text("secret");
        t.enc.key = "right".into();
        t.encrypt();
        t.send_to_decrypt();
        let good = t.dec.output.text();
        assert_eq!(good, "secret");

        t.dec.key = "wrong".into();
        t.decrypt();
        assert!(!t.dec.ok, "错口令必须报错");
        assert_eq!(t.dec.output.text(), good, "失败时不该清掉上次的明文");
    }

    #[test]
    fn every_algo_round_trips() {
        // 八个算法都要能自洽往返；漏一个的表现是「某个算法一用就报错」。
        for (i, algo) in Algo::ALL.iter().enumerate() {
            let mut t = CryptoTool::default();
            t.set_enc_algo(i as i32);
            t.enc.input.set_text("payload-中文-123");
            t.enc.key = "pw".into();
            t.encrypt();
            assert!(t.enc.ok, "{:?} 加密失败：{}", algo, t.enc.status);
            t.send_to_decrypt();
            assert!(t.dec.ok, "{:?} 解密失败：{}", algo, t.dec.status);
            assert_eq!(
                t.dec.output.text(),
                "payload-中文-123",
                "{algo:?} 往返不一致"
            );
        }
    }

    #[test]
    fn empty_input_is_a_hint_not_an_error() {
        let mut t = CryptoTool::default();
        t.encrypt();
        assert!(t.enc.ok, "空输入不该显示成红字错误");
        assert!(t.enc.status.contains("请输入"));
        t.decrypt();
        assert!(t.dec.ok);
        assert!(t.dec.status.contains("请粘入"));
    }

    #[test]
    fn draft_never_contains_the_password() {
        // app.ron 是当前用户可写的普通文件。
        let mut t = CryptoTool::default();
        t.enc.key = "SUPERSECRET".into();
        t.dec.key = "ALSOSECRET".into();
        t.enc.input.set_text("hello");
        let saved = t.save_draft().expect("必须持久化草稿");
        assert!(!saved.contains("SUPERSECRET"), "草稿里出现了口令：{saved}");
        assert!(!saved.contains("ALSOSECRET"), "草稿里出现了口令：{saved}");
        assert!(saved.contains("hello"), "输入文本还是要存的");
    }

    #[test]
    fn draft_roundtrip_preserves_inputs_and_algos() {
        let mut t = CryptoTool::default();
        // 索引按 Algo::ALL 的顺序（…TripleDes, Rabbit, Rc4）
        t.set_enc_algo(5); // TripleDes
        t.set_dec_algo(7); // Rc4
        t.enc.input.set_text("a");
        t.dec.input.set_text("b");
        let saved = t.save_draft().unwrap();

        let mut r = CryptoTool::default();
        r.load_draft(&saved);
        assert_eq!(r.enc.algo, Algo::TripleDes);
        assert_eq!(r.dec.algo, Algo::Rc4);
        assert_eq!(r.enc.input.text(), "a");
        assert_eq!(r.dec.input.text(), "b");
        assert!(r.enc.key.is_empty(), "口令不该从草稿里恢复出来");
    }

    #[test]
    fn legacy_aes_alias_still_loads() {
        // 旧版草稿把 AES-CBC 存成 "Aes"。改名会让老用户的算法选择回默认。
        let mut t = CryptoTool::default();
        t.load_draft(r#"{"enc_text":"x","dec_text":"","enc_algo":"Aes","dec_algo":"Aes"}"#);
        assert_eq!(t.enc.algo, Algo::AesCbc);
        assert_eq!(t.dec.algo, Algo::AesCbc);
    }

    #[test]
    fn out_of_range_algo_index_is_ignored() {
        let mut t = CryptoTool::default();
        t.set_enc_algo(999);
        t.set_dec_algo(-5);
        assert_eq!(t.enc.algo, Algo::AesCbc);
        assert_eq!(t.dec.algo, Algo::AesCbc);
    }

    #[test]
    fn algo_labels_cover_every_variant() {
        assert_eq!(CryptoTool::algo_labels().len(), Algo::ALL.len());
    }
}
