//! 国密 SM —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `GmView`，三块：
//!
//! 1. **SM2 密钥对**：一键生成，公钥 / 私钥各一栏；
//! 2. **加密 / 解密**：SM4（ECB/CBC/CTR/CFB/OFB）与 SM2 公钥加密、SM3 摘要；
//! 3. **SM2 签名 / 验签**。
//!
//! 算法与实现全在 `ferric_core::gm`（smcrypto），没动过。
//!
//! ⚠️ 二进制里同时存在 `smcrypto`（本工具）与 `libsm`（更新传输层），
//! 两者默认线格式不兼容 —— 别为了「统一一下」把任一边换掉。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::gm::{self, DecAlgo, EncAlgo, Sm2Fmt};
use serde::{Deserialize, Serialize};

fn default_sm2_fmt() -> Sm2Fmt {
    Sm2Fmt::C1C3C2
}

#[derive(Serialize, Deserialize)]
struct GmDraft {
    pub_key: String,
    priv_key: String,
    enc_text: String,
    dec_text: String,
    enc_algo: EncAlgo,
    dec_algo: DecAlgo,
    sig_text: String,
    /// 旧版草稿没有这一项 —— 回落到 C1C3C2（国标默认排布），
    /// 与 egui 版的初值一致。
    #[serde(default = "default_sm2_fmt")]
    sm2_fmt: Sm2Fmt,
}

pub struct GmTool {
    // ——— SM2 密钥对 ———
    pub pub_key: String,
    pub priv_key: String,
    pub key_status: String,

    // ——— 加密 ———
    pub enc_algo: EncAlgo,
    pub enc_key: String,
    pub enc_input: TextBuffer,
    pub enc_output: TextBuffer,
    pub enc_ok: bool,
    pub enc_status: String,

    // ——— 解密 ———
    pub dec_algo: DecAlgo,
    pub dec_key: String,
    pub dec_input: TextBuffer,
    pub dec_output: TextBuffer,
    pub dec_ok: bool,
    pub dec_status: String,

    // ——— SM2 密文格式（三种跨实现常见排布） ———
    pub sm2_fmt: Sm2Fmt,

    // ——— 签名 / 验签 ———
    pub sig_text: TextBuffer,
    pub sig_hex: String,
    pub sig_ok: bool,
    pub sig_status: String,
}

impl Default for GmTool {
    fn default() -> Self {
        Self {
            pub_key: String::new(),
            priv_key: String::new(),
            key_status: "点「生成密钥对」开始".to_owned(),
            enc_algo: EncAlgo::Sm4Ecb,
            enc_key: String::new(),
            enc_input: TextBuffer::new(""),
            enc_output: TextBuffer::new(""),
            enc_ok: true,
            enc_status: "就绪".to_owned(),
            dec_algo: DecAlgo::Sm4Ecb,
            dec_key: String::new(),
            dec_input: TextBuffer::new(""),
            dec_output: TextBuffer::new(""),
            dec_ok: true,
            dec_status: "就绪".to_owned(),
            sm2_fmt: Sm2Fmt::C1C3C2,
            sig_text: TextBuffer::new(""),
            sig_hex: String::new(),
            sig_ok: true,
            sig_status: "就绪".to_owned(),
        }
    }
}

impl GmTool {
    pub fn enc_algo_labels() -> Vec<&'static str> {
        EncAlgo::ALL.iter().map(|a| a.label()).collect()
    }
    pub fn dec_algo_labels() -> Vec<&'static str> {
        DecAlgo::ALL.iter().map(|a| a.label()).collect()
    }
    pub fn fmt_labels() -> Vec<&'static str> {
        Sm2Fmt::ALL.iter().map(|f| f.label()).collect()
    }

    pub fn enc_algo_index(&self) -> i32 {
        EncAlgo::ALL
            .iter()
            .position(|a| *a == self.enc_algo)
            .unwrap_or(0) as i32
    }
    pub fn dec_algo_index(&self) -> i32 {
        DecAlgo::ALL
            .iter()
            .position(|a| *a == self.dec_algo)
            .unwrap_or(0) as i32
    }
    pub fn fmt_index(&self) -> i32 {
        Sm2Fmt::ALL
            .iter()
            .position(|f| *f == self.sm2_fmt)
            .unwrap_or(0) as i32
    }

    pub fn set_enc_algo(&mut self, i: i32) {
        if let Some(a) = EncAlgo::ALL.get(i.max(0) as usize) {
            self.enc_algo = *a;
            self.enc_status = format!("算法：{}", a.label());
        }
    }
    pub fn set_dec_algo(&mut self, i: i32) {
        if let Some(a) = DecAlgo::ALL.get(i.max(0) as usize) {
            self.dec_algo = *a;
            self.dec_status = format!("算法：{}", a.label());
        }
    }
    pub fn set_fmt(&mut self, i: i32) {
        if let Some(f) = Sm2Fmt::ALL.get(i.max(0) as usize) {
            self.sm2_fmt = *f;
        }
    }

    /// SM2 加解密时口令栏放的是公钥 / 私钥，其余算法放的是口令。
    /// 这个判断决定界面上那一栏的标签文案。
    pub fn enc_key_is_pubkey(&self) -> bool {
        self.enc_algo == EncAlgo::Sm2
    }
    pub fn dec_key_is_privkey(&self) -> bool {
        self.dec_algo == DecAlgo::Sm2
    }
    /// SM3 是摘要，没有口令可言。
    pub fn enc_needs_key(&self) -> bool {
        self.enc_algo != EncAlgo::Sm3
    }

    pub fn gen_keypair(&mut self) {
        // 注意返回顺序是 (公钥, 私钥) —— 见 gm::gen_sm2_keypair 的文档。
        // 写反的表现是「签名说私钥无效」，而错在这一行。
        let (pk, sk) = gm::gen_sm2_keypair();
        self.priv_key = sk;
        self.pub_key = pk;
        self.key_status = "已生成 SM2 密钥对".to_owned();
    }

    /// 从私钥推出公钥（用户手上只有私钥时）。
    pub fn derive_pub(&mut self) {
        if self.priv_key.trim().is_empty() {
            self.key_status = "请先填入私钥".to_owned();
            return;
        }
        match gm::sm2_pk_from_sk(self.priv_key.trim()) {
            Ok(pk) => {
                self.pub_key = pk;
                self.key_status = "已由私钥推出公钥".to_owned();
            }
            Err(e) => self.key_status = format!("推导失败：{e}"),
        }
    }

    pub fn encrypt(&mut self) {
        let text = self.enc_input.text();
        if text.is_empty() {
            self.enc_ok = true;
            self.enc_status = "请输入要处理的文本".to_owned();
            return;
        }
        // SM2 走带格式的接口（C1C3C2 / C1C2C3 / ASN.1 三种排布跨实现常见），
        // 其余算法走通用接口。
        let r = if self.enc_algo == EncAlgo::Sm2 {
            gm::sm2_encrypt_fmt(self.sm2_fmt, &text, self.enc_key.trim())
        } else {
            gm::encrypt(self.enc_algo, &text, &self.enc_key)
        };
        match r {
            Ok(out) => {
                self.enc_output.set_text(&out);
                self.enc_ok = true;
                self.enc_status = format!("已处理（{}）", self.enc_algo.label());
            }
            Err(e) => {
                self.enc_ok = false;
                self.enc_status = format!("失败：{e}");
            }
        }
    }

    pub fn decrypt(&mut self) {
        let text = self.dec_input.text();
        if text.trim().is_empty() {
            self.dec_ok = true;
            self.dec_status = "请粘入密文（hex）".to_owned();
            return;
        }
        let r = if self.dec_algo == DecAlgo::Sm2 {
            gm::sm2_decrypt_fmt(self.sm2_fmt, text.trim(), self.dec_key.trim())
        } else {
            gm::decrypt(self.dec_algo, text.trim(), &self.dec_key)
        };
        match r {
            Ok(out) => {
                self.dec_output.set_text(&out);
                self.dec_ok = true;
                self.dec_status = format!("已解密（{}）", self.dec_algo.label());
            }
            Err(e) => {
                // 不清空上一次的明文 —— 那可能是用户还要用的结果。
                self.dec_ok = false;
                self.dec_status = format!("解密失败：{e}");
            }
        }
    }

    /// 把加密结果送去解密。SM2 时连格式一起带（三种排布互不兼容，
    /// 只搬密文必然报错），SM4 时把算法对齐过去。
    pub fn send_to_decrypt(&mut self) {
        let cipher = self.enc_output.text();
        if cipher.is_empty() {
            return;
        }
        self.dec_algo = match self.enc_algo {
            EncAlgo::Sm4Ecb => DecAlgo::Sm4Ecb,
            EncAlgo::Sm4Cbc => DecAlgo::Sm4Cbc,
            EncAlgo::Sm4Ctr => DecAlgo::Sm4Ctr,
            EncAlgo::Sm4Cfb => DecAlgo::Sm4Cfb,
            EncAlgo::Sm4Ofb => DecAlgo::Sm4Ofb,
            EncAlgo::Sm2 => DecAlgo::Sm2,
            // SM3 是摘要，不可逆。搬过去只会让用户对着一条报错发呆。
            EncAlgo::Sm3 => {
                self.dec_status = "SM3 是摘要，不可解密".to_owned();
                self.dec_ok = false;
                return;
            }
        };
        self.dec_key = if self.enc_algo == EncAlgo::Sm2 {
            self.priv_key.clone()
        } else {
            self.enc_key.clone()
        };
        self.dec_input.set_text(&cipher);
        self.decrypt();
    }

    pub fn sign(&mut self) {
        let text = self.sig_text.text();
        if text.is_empty() {
            self.sig_ok = true;
            self.sig_status = "请输入要签名的文本".to_owned();
            return;
        }
        match gm::sm2_sign(&text, self.priv_key.trim()) {
            Ok(sig) => {
                self.sig_hex = sig;
                self.sig_ok = true;
                self.sig_status = "已签名".to_owned();
            }
            Err(e) => {
                self.sig_ok = false;
                self.sig_status = format!("签名失败：{e}");
            }
        }
    }

    pub fn verify(&mut self) {
        let text = self.sig_text.text();
        match gm::sm2_verify(&text, self.sig_hex.trim(), self.pub_key.trim()) {
            // 验签**失败不是错误**，是一个有效结论：这份签名对不上。
            // 用红字报「失败」会让人以为程序出错了。
            Ok(true) => {
                self.sig_ok = true;
                self.sig_status = "验签通过 ✓".to_owned();
            }
            Ok(false) => {
                self.sig_ok = false;
                self.sig_status = "验签不通过 —— 签名与文本/公钥不匹配".to_owned();
            }
            Err(e) => {
                self.sig_ok = false;
                self.sig_status = format!("无法验签：{e}");
            }
        }
    }
}

impl Tool for GmTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "gm",
            name: "国密 SM",
            desc: "SM4（ECB/CBC/CTR/CFB/OFB）对称、SM2 公钥加解密与签名验签、SM3 摘要。",
            icon: icons::SHIELD_CHECK,
            group: "加密",
            keywords: &["gm", "国密", "sm2", "sm3", "sm4", "国密sm"],
        }
    }

    /// 存输入、算法与**密钥对**。
    ///
    /// 与 `crypto` 工具的口令不同：SM2 密钥对是用户显式生成、要反复用的
    /// 长期材料，丢了就没法解开之前的密文 —— egui 版也是存的。
    /// 对称口令（`enc_key` / `dec_key`）仍然不存。
    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&GmDraft {
            pub_key: self.pub_key.clone(),
            priv_key: self.priv_key.clone(),
            enc_text: self.enc_input.text(),
            dec_text: self.dec_input.text(),
            enc_algo: self.enc_algo,
            dec_algo: self.dec_algo,
            sig_text: self.sig_text.text(),
            sm2_fmt: self.sm2_fmt,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<GmDraft>(data) {
            self.pub_key = d.pub_key;
            self.priv_key = d.priv_key;
            self.enc_input.set_text(&d.enc_text);
            self.dec_input.set_text(&d.dec_text);
            self.enc_algo = d.enc_algo;
            self.dec_algo = d.dec_algo;
            self.sig_text.set_text(&d.sig_text);
            self.sm2_fmt = d.sm2_fmt;
            if !self.priv_key.is_empty() {
                self.key_status = "已从草稿恢复密钥对".to_owned();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_generation_fills_both_halves() {
        let mut t = GmTool::default();
        t.gen_keypair();
        assert!(!t.priv_key.is_empty());
        assert!(!t.pub_key.is_empty());
        assert!(t.key_status.contains("已生成"));
    }

    #[test]
    fn public_key_can_be_derived_from_private() {
        let mut t = GmTool::default();
        t.gen_keypair();
        let pk = t.pub_key.clone();
        t.pub_key.clear();
        t.derive_pub();
        assert_eq!(t.pub_key, pk, "由私钥推出的公钥应与生成时一致");
    }

    #[test]
    fn deriving_without_a_private_key_says_so() {
        let mut t = GmTool::default();
        t.derive_pub();
        assert!(t.key_status.contains("请先填入私钥"));
    }

    #[test]
    fn every_sm4_mode_round_trips() {
        // 五个 SM4 模式都要能自洽往返。
        for (i, algo) in EncAlgo::ALL.iter().enumerate() {
            if matches!(algo, EncAlgo::Sm2 | EncAlgo::Sm3) {
                continue;
            }
            let mut t = GmTool::default();
            t.set_enc_algo(i as i32);
            t.enc_key = "0123456789abcdef".into();
            t.enc_input.set_text("国密载荷-payload-123");
            t.encrypt();
            assert!(t.enc_ok, "{algo:?} 加密失败：{}", t.enc_status);
            t.send_to_decrypt();
            assert!(t.dec_ok, "{algo:?} 解密失败：{}", t.dec_status);
            assert_eq!(
                t.dec_output.text(),
                "国密载荷-payload-123",
                "{algo:?} 往返不一致"
            );
        }
    }

    #[test]
    fn sm2_round_trips_in_every_wire_format() {
        // 三种密文排布互不兼容。逐个验证，否则「换个格式就解不开」。
        for (i, fmt) in Sm2Fmt::ALL.iter().enumerate() {
            let mut t = GmTool::default();
            t.gen_keypair();
            t.set_fmt(i as i32);
            t.set_enc_algo(
                EncAlgo::ALL
                    .iter()
                    .position(|a| *a == EncAlgo::Sm2)
                    .unwrap() as i32,
            );
            t.enc_key = t.pub_key.clone();
            t.enc_input.set_text("sm2 载荷");
            t.encrypt();
            assert!(t.enc_ok, "{fmt:?} 加密失败：{}", t.enc_status);
            t.send_to_decrypt();
            assert!(t.dec_ok, "{fmt:?} 解密失败：{}", t.dec_status);
            assert_eq!(t.dec_output.text(), "sm2 载荷", "{fmt:?} 往返不一致");
        }
    }

    #[test]
    fn send_to_decrypt_refuses_sm3_instead_of_producing_a_confusing_error() {
        // SM3 是摘要，不可逆。搬过去只会让用户对着一条底层报错发呆。
        let mut t = GmTool::default();
        t.set_enc_algo(
            EncAlgo::ALL
                .iter()
                .position(|a| *a == EncAlgo::Sm3)
                .unwrap() as i32,
        );
        t.enc_input.set_text("x");
        t.encrypt();
        assert!(t.enc_ok, "{}", t.enc_status);
        t.send_to_decrypt();
        assert!(!t.dec_ok);
        assert!(t.dec_status.contains("不可解密"), "{}", t.dec_status);
    }

    #[test]
    fn sm3_is_a_digest_and_needs_no_key() {
        let mut t = GmTool::default();
        t.set_enc_algo(
            EncAlgo::ALL
                .iter()
                .position(|a| *a == EncAlgo::Sm3)
                .unwrap() as i32,
        );
        assert!(!t.enc_needs_key(), "SM3 不该显示口令栏");
        t.enc_input.set_text("abc");
        t.encrypt();
        assert!(t.enc_ok, "{}", t.enc_status);
        // SM3 摘要是 256 位 = 64 个 hex 字符
        assert_eq!(t.enc_output.text().trim().len(), 64);
    }

    #[test]
    fn sign_then_verify_passes_and_tampering_fails() {
        let mut t = GmTool::default();
        t.gen_keypair();
        t.sig_text.set_text("待签名内容");
        t.sign();
        assert!(t.sig_ok, "{}", t.sig_status);
        t.verify();
        assert!(t.sig_ok, "{}", t.sig_status);
        assert!(t.sig_status.contains("通过"));

        // 改一个字 → 验签必须不通过
        t.sig_text.set_text("待签名内容!");
        t.verify();
        assert!(!t.sig_ok);
        assert!(t.sig_status.contains("不通过"), "{}", t.sig_status);
    }

    #[test]
    fn verification_failure_is_a_conclusion_not_a_crash() {
        // 验签不通过是有效结论。这里守的是「不要 panic、也不要报成程序错误」。
        let mut t = GmTool::default();
        t.gen_keypair();
        t.sig_text.set_text("x");
        t.sig_hex = "00".repeat(64);
        t.verify();
        assert!(!t.sig_ok);
        assert!(!t.sig_status.is_empty());
    }

    #[test]
    fn draft_keeps_the_keypair_but_not_the_symmetric_password() {
        // 密钥对是用户要反复用的长期材料（丢了解不开旧密文）；
        // 对称口令是一次性的，不存。
        let mut t = GmTool::default();
        t.gen_keypair();
        t.enc_key = "SYMPASSWORD".into();
        t.dec_key = "SYMPASSWORD2".into();
        let saved = t.save_draft().expect("必须持久化草稿");
        assert!(saved.contains(&t.priv_key), "密钥对应当持久化");
        assert!(!saved.contains("SYMPASSWORD"), "对称口令不该落盘：{saved}");
    }

    #[test]
    fn draft_roundtrip_preserves_algos_and_format() {
        let mut t = GmTool::default();
        t.gen_keypair();
        t.set_enc_algo(1); // Sm4Cbc
        t.set_dec_algo(2); // Sm4Ctr
        t.set_fmt(2); // Asn1
        t.enc_input.set_text("a");
        let saved = t.save_draft().unwrap();

        let mut r = GmTool::default();
        r.load_draft(&saved);
        assert_eq!(r.enc_algo, EncAlgo::Sm4Cbc);
        assert_eq!(r.dec_algo, DecAlgo::Sm4Ctr);
        assert_eq!(r.sm2_fmt, Sm2Fmt::Asn1);
        assert_eq!(r.priv_key, t.priv_key);
    }

    #[test]
    fn legacy_sm4_alias_still_loads() {
        // 旧版草稿把 SM4-ECB 存成 "Sm4"。
        let mut t = GmTool::default();
        t.load_draft(
            r#"{"pub_key":"","priv_key":"","enc_text":"x","dec_text":"","enc_algo":"Sm4","dec_algo":"Sm4","sig_text":""}"#,
        );
        assert_eq!(t.enc_algo, EncAlgo::Sm4Ecb);
        assert_eq!(t.dec_algo, DecAlgo::Sm4Ecb);
    }

    #[test]
    fn out_of_range_indices_are_ignored() {
        let mut t = GmTool::default();
        t.set_enc_algo(99);
        t.set_dec_algo(-1);
        t.set_fmt(42);
        assert_eq!(t.enc_algo, EncAlgo::Sm4Ecb);
        assert_eq!(t.dec_algo, DecAlgo::Sm4Ecb);
        assert_eq!(t.sm2_fmt, Sm2Fmt::C1C3C2);
    }
}
