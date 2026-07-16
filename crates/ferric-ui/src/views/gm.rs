//! 国密 SM 加解密视图（SM2 / SM3 / SM4）。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{ComboBox, RichText, TextEdit, Ui};
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
    #[serde(default = "default_sm2_fmt")]
    sm2_fmt_enc: Sm2Fmt,
    #[serde(default = "default_sm2_fmt")]
    sm2_fmt_dec: Sm2Fmt,
    #[serde(default)]
    sig_text: String,
}

pub struct GmTool {
    pub_key: String,
    priv_key: String,
    enc_text: String,
    enc_key: String,
    enc_out: String,
    enc_algo: EncAlgo,
    enc_ok: bool,
    enc_status: String,
    dec_text: String,
    dec_key: String,
    dec_out: String,
    dec_algo: DecAlgo,
    dec_ok: bool,
    dec_status: String,
    /// SM2 密文格式（加密 / 解密各自独立选择）。
    sm2_fmt_enc: Sm2Fmt,
    sm2_fmt_dec: Sm2Fmt,
    /// SM2 签名 / 验签。
    sig_text: String,
    sig_hex: String,
    sig_ok: bool,
    sig_status: String,
}

impl Default for GmTool {
    fn default() -> Self {
        Self {
            pub_key: String::new(),
            priv_key: String::new(),
            enc_text: String::new(),
            enc_key: String::new(),
            enc_out: String::new(),
            enc_algo: EncAlgo::Sm4Ecb,
            enc_ok: true,
            enc_status: "就绪".to_owned(),
            dec_text: String::new(),
            dec_key: String::new(),
            dec_out: String::new(),
            dec_algo: DecAlgo::Sm4Ecb,
            dec_ok: true,
            dec_status: "就绪".to_owned(),
            sm2_fmt_enc: default_sm2_fmt(),
            sm2_fmt_dec: default_sm2_fmt(),
            sig_text: String::new(),
            sig_hex: String::new(),
            sig_ok: true,
            sig_status: "就绪".to_owned(),
        }
    }
}

impl Tool for GmTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "gm",
            name: "国密 SM 加解密",
            group: "加密",
            desc: "使用国密算法加密和解密文本 —— SM4（对称 · ECB/CBC）、SM2（非对称公钥）、SM3（摘要）。",
            icon: icons::SHIELD_CHECK,
            keywords: &["gm", "国密", "sm2", "sm3", "sm4", "国密sm"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 卡1：SM2 密钥对
        widgets::card(ui, &theme, |ui| {
            ui.label(
                RichText::new("SM2 密钥对")
                    .size(15.0)
                    .strong()
                    .color(theme.fg),
            );
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "公钥（加密用 · 04 开头 130 hex）");
                cols[0].add_space(4.0);
                widgets::code_area(&mut cols[0], "gm-pub", &mut self.pub_key, true, 3);
                widgets::field_label(&mut cols[1], &theme, "私钥（解密用 · 64 hex）");
                cols[1].add_space(4.0);
                widgets::code_area(&mut cols[1], "gm-priv", &mut self.priv_key, true, 3);
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if widgets::subtle_button(ui, &theme, Some(icons::REFRESH_CW), "生成 SM2 密钥对")
                    .clicked()
                {
                    let (pk, sk) = gm::gen_sm2_keypair();
                    self.pub_key = pk;
                    self.priv_key = sk;
                }
                if widgets::subtle_button(ui, &theme, Some(icons::KEY), "由私钥反推公钥").clicked()
                {
                    match gm::sm2_pk_from_sk(&self.priv_key) {
                        Ok(pk) => {
                            self.pub_key = pk;
                            shared.toast("已由私钥反推出公钥");
                        }
                        Err(e) => shared.toast(e),
                    }
                }
                if widgets::subtle_button(ui, &theme, Some(icons::FILE_DOWN), "导出 PEM").clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("PEM", &["pem"])
                        .set_file_name("sm2-keypair.pem")
                        .save_file()
                    {
                        match gm::sm2_keypair_to_pem(&self.priv_key, &path.to_string_lossy()) {
                            Ok(()) => shared.toast("已导出 PEM（含私钥，注意保管）"),
                            Err(e) => shared.toast(e),
                        }
                    }
                }
                if widgets::subtle_button(ui, &theme, Some(icons::FOLDER_OPEN), "导入 PEM")
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("PEM", &["pem"])
                        .pick_file()
                    {
                        match gm::sm2_keypair_from_pem(&path.to_string_lossy()) {
                            Ok((pk, sk)) => {
                                self.pub_key = pk;
                                self.priv_key = sk;
                                shared.toast("已从 PEM 导入密钥对");
                            }
                            Err(e) => shared.toast(e),
                        }
                    }
                }
                ui.label(
                    RichText::new("仅 SM2 需要；SM4 用口令，SM3 为单向摘要")
                        .size(11.5)
                        .color(theme.faint),
                );
            });
        });
        ui.add_space(14.0);

        // 卡2：加密 / 摘要
        widgets::card(ui, &theme, |ui| {
            ui.label(
                RichText::new("加密 / 摘要")
                    .size(15.0)
                    .strong()
                    .color(theme.fg),
            );
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "您的文本");
                cols[0].add_space(4.0);
                widgets::code_area(&mut cols[0], "gm-enc-text", &mut self.enc_text, true, 5);
                widgets::field_label(&mut cols[1], &theme, "您的密钥 / 口令");
                cols[1].add_space(4.0);
                let hint = match self.enc_algo {
                    EncAlgo::Sm4Ecb
                    | EncAlgo::Sm4Cbc
                    | EncAlgo::Sm4Ctr
                    | EncAlgo::Sm4Cfb
                    | EncAlgo::Sm4Ofb => "SM4 口令",
                    EncAlgo::Sm2 => "SM2 公钥（留空则用上方密钥对）",
                    EncAlgo::Sm3 => "SM3 摘要无需密钥",
                };
                cols[1].add_enabled(
                    self.enc_algo != EncAlgo::Sm3,
                    TextEdit::singleline(&mut self.enc_key)
                        .desired_width(f32::INFINITY)
                        .hint_text(hint),
                );
                cols[1].add_space(8.0);
                widgets::field_label(&mut cols[1], &theme, "国密算法");
                cols[1].add_space(4.0);
                ComboBox::from_id_salt("gm-enc-algo")
                    .selected_text(self.enc_algo.label())
                    .show_ui(&mut cols[1], |ui| {
                        for a in EncAlgo::ALL {
                            ui.selectable_value(&mut self.enc_algo, a, a.label());
                        }
                    });
                if self.enc_algo == EncAlgo::Sm2 {
                    cols[1].add_space(8.0);
                    widgets::field_label(&mut cols[1], &theme, "SM2 密文格式");
                    cols[1].add_space(4.0);
                    ComboBox::from_id_salt("gm-enc-sm2fmt")
                        .selected_text(self.sm2_fmt_enc.label())
                        .show_ui(&mut cols[1], |ui| {
                            for f in Sm2Fmt::ALL {
                                ui.selectable_value(&mut self.sm2_fmt_enc, f, f.label());
                            }
                        });
                }
            });
            ui.add_space(10.0);
            // SM2 优先用本卡输入的公钥，留空才回落到卡1生成的密钥对。
            let key_for_enc = if self.enc_algo == EncAlgo::Sm2 && self.enc_key.trim().is_empty() {
                self.pub_key.clone()
            } else {
                self.enc_key.clone()
            };
            ui.horizontal(|ui| {
                if widgets::primary_icon(ui, &theme, icons::LOCK, "加密 / 摘要").clicked() {
                    let res = if self.enc_algo == EncAlgo::Sm2 {
                        gm::sm2_encrypt_fmt(self.sm2_fmt_enc, &self.enc_text, &key_for_enc)
                    } else {
                        gm::encrypt(self.enc_algo, &self.enc_text, &key_for_enc)
                    };
                    match res {
                        Ok(out) => {
                            self.enc_out = out;
                            self.enc_ok = true;
                            self.enc_status = "完成".to_owned();
                        }
                        Err(e) => {
                            self.enc_out.clear();
                            self.enc_ok = false;
                            self.enc_status = format!("失败：{e}");
                        }
                    }
                }
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制").clicked() {
                    shared.copy(ui.ctx(), self.enc_out.clone());
                }
                ui.add_space(6.0);
                widgets::status_line(ui, &theme, self.enc_ok, &self.enc_status);
            });
            ui.add_space(8.0);
            widgets::field_label(ui, &theme, "加密 / 摘要结果（hex）");
            ui.add_space(4.0);
            widgets::code_area(ui, "gm-enc-out", &mut self.enc_out, false, 3);
        });
        ui.add_space(14.0);

        // 卡3：解密
        widgets::card(ui, &theme, |ui| {
            ui.label(RichText::new("解密").size(15.0).strong().color(theme.fg));
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "您的密文（hex）");
                cols[0].add_space(4.0);
                widgets::code_area(&mut cols[0], "gm-dec-text", &mut self.dec_text, true, 5);
                widgets::field_label(&mut cols[1], &theme, "您的密钥 / 口令");
                cols[1].add_space(4.0);
                let hint = match self.dec_algo {
                    DecAlgo::Sm4Ecb
                    | DecAlgo::Sm4Cbc
                    | DecAlgo::Sm4Ctr
                    | DecAlgo::Sm4Cfb
                    | DecAlgo::Sm4Ofb => "SM4 口令",
                    DecAlgo::Sm2 => "SM2 私钥（留空则用上方密钥对）",
                };
                cols[1].add(
                    TextEdit::singleline(&mut self.dec_key)
                        .desired_width(f32::INFINITY)
                        .hint_text(hint),
                );
                cols[1].add_space(8.0);
                widgets::field_label(&mut cols[1], &theme, "国密算法");
                cols[1].add_space(4.0);
                ComboBox::from_id_salt("gm-dec-algo")
                    .selected_text(self.dec_algo.label())
                    .show_ui(&mut cols[1], |ui| {
                        for a in DecAlgo::ALL {
                            ui.selectable_value(&mut self.dec_algo, a, a.label());
                        }
                    });
                if self.dec_algo == DecAlgo::Sm2 {
                    cols[1].add_space(8.0);
                    widgets::field_label(&mut cols[1], &theme, "SM2 密文格式");
                    cols[1].add_space(4.0);
                    ComboBox::from_id_salt("gm-dec-sm2fmt")
                        .selected_text(self.sm2_fmt_dec.label())
                        .show_ui(&mut cols[1], |ui| {
                            for f in Sm2Fmt::ALL {
                                ui.selectable_value(&mut self.sm2_fmt_dec, f, f.label());
                            }
                        });
                }
            });
            ui.add_space(10.0);
            // SM2 优先用本卡输入的私钥，留空才回落到卡1生成的密钥对。
            let key_for_dec = if self.dec_algo == DecAlgo::Sm2 && self.dec_key.trim().is_empty() {
                self.priv_key.clone()
            } else {
                self.dec_key.clone()
            };
            ui.horizontal(|ui| {
                if widgets::primary_icon(ui, &theme, icons::KEY, "解密").clicked() {
                    let res = if self.dec_algo == DecAlgo::Sm2 {
                        gm::sm2_decrypt_fmt(self.sm2_fmt_dec, &self.dec_text, &key_for_dec)
                    } else {
                        gm::decrypt(self.dec_algo, &self.dec_text, &key_for_dec)
                    };
                    match res {
                        Ok(out) => {
                            self.dec_out = out;
                            self.dec_ok = true;
                            self.dec_status = "已解密".to_owned();
                        }
                        Err(e) => {
                            self.dec_out.clear();
                            self.dec_ok = false;
                            self.dec_status = format!("解密失败：{e}");
                        }
                    }
                }
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制").clicked() {
                    shared.copy(ui.ctx(), self.dec_out.clone());
                }
                ui.add_space(6.0);
                widgets::status_line(ui, &theme, self.dec_ok, &self.dec_status);
            });
            ui.add_space(8.0);
            widgets::field_label(ui, &theme, "您的解密文本");
            ui.add_space(4.0);
            widgets::code_area(ui, "gm-dec-out", &mut self.dec_out, false, 3);
        });
        ui.add_space(14.0);

        // 卡4：SM2 签名 / 验签（使用卡1的密钥对：私钥签名、公钥验签）
        widgets::card(ui, &theme, |ui| {
            ui.label(
                RichText::new("SM2 签名 / 验签")
                    .size(15.0)
                    .strong()
                    .color(theme.fg),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("使用上方密钥对：私钥签名，公钥验签；SM3 摘要 + 默认 ID，签名输出 ASN.1/DER hex")
                    .size(11.5)
                    .color(theme.faint),
            );
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "原文");
                cols[0].add_space(4.0);
                widgets::code_area(&mut cols[0], "gm-sig-text", &mut self.sig_text, true, 4);
                widgets::field_label(&mut cols[1], &theme, "签名值（hex，可粘贴待验签名）");
                cols[1].add_space(4.0);
                widgets::code_area(&mut cols[1], "gm-sig-hex", &mut self.sig_hex, true, 4);
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary_icon(ui, &theme, icons::LOCK, "签名").clicked() {
                    match gm::sm2_sign(&self.sig_text, &self.priv_key) {
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
                if widgets::ghost_button(ui, &theme, "验签").clicked() {
                    match gm::sm2_verify(&self.sig_text, &self.sig_hex, &self.pub_key) {
                        Ok(true) => {
                            self.sig_ok = true;
                            self.sig_status = "验签通过：签名有效".to_owned();
                        }
                        Ok(false) => {
                            self.sig_ok = false;
                            self.sig_status = "验签不通过：签名与原文/公钥不匹配".to_owned();
                        }
                        Err(e) => {
                            self.sig_ok = false;
                            self.sig_status = format!("验签失败：{e}");
                        }
                    }
                }
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制签名").clicked() {
                    shared.copy(ui.ctx(), self.sig_hex.clone());
                }
                ui.add_space(6.0);
                widgets::status_line(ui, &theme, self.sig_ok, &self.sig_status);
            });
        });
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&GmDraft {
            pub_key: self.pub_key.clone(),
            priv_key: self.priv_key.clone(),
            enc_text: self.enc_text.clone(),
            dec_text: self.dec_text.clone(),
            enc_algo: self.enc_algo,
            dec_algo: self.dec_algo,
            sm2_fmt_enc: self.sm2_fmt_enc,
            sm2_fmt_dec: self.sm2_fmt_dec,
            sig_text: self.sig_text.clone(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<GmDraft>(data) {
            self.pub_key = d.pub_key;
            self.priv_key = d.priv_key;
            self.enc_text = d.enc_text;
            self.dec_text = d.dec_text;
            self.enc_algo = d.enc_algo;
            self.dec_algo = d.dec_algo;
            self.sm2_fmt_enc = d.sm2_fmt_enc;
            self.sm2_fmt_dec = d.sm2_fmt_dec;
            self.sig_text = d.sig_text;
        }
    }
}
