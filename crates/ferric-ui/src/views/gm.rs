//! 国密 SM 加解密视图（SM2 / SM3 / SM4）。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{ComboBox, RichText, TextEdit, Ui};
use ferric_core::gm::{self, DecAlgo, EncAlgo};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct GmDraft {
    pub_key: String,
    priv_key: String,
    enc_text: String,
    dec_text: String,
    enc_algo: EncAlgo,
    dec_algo: DecAlgo,
}

pub struct GmTool {
    pub_key: String,
    priv_key: String,
    enc_text: String,
    enc_key: String,
    enc_out: String,
    enc_algo: EncAlgo,
    dec_text: String,
    dec_key: String,
    dec_out: String,
    dec_algo: DecAlgo,
}

impl Default for GmTool {
    fn default() -> Self {
        Self {
            pub_key: String::new(),
            priv_key: String::new(),
            enc_text: String::new(),
            enc_key: String::new(),
            enc_out: String::new(),
            enc_algo: EncAlgo::Sm4,
            dec_text: String::new(),
            dec_key: String::new(),
            dec_out: String::new(),
            dec_algo: DecAlgo::Sm4,
        }
    }
}

impl Tool for GmTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "gm",
            name: "国密 SM 加解密",
            group: "加密",
            desc: "使用国密算法加密和解密文本 —— SM4（对称）、SM2（非对称公钥）、SM3（摘要）。",
            icon: icons::SHIELD_CHECK,
            keywords: &["gm", "国密", "sm2", "sm3", "sm4", "国密sm"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 卡1：SM2 密钥对
        widgets::card(ui, &theme, |ui| {
            ui.label(RichText::new("SM2 密钥对").size(15.0).strong().color(theme.fg));
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
            ui.horizontal(|ui| {
                if widgets::subtle_button(ui, &theme, Some(icons::REFRESH_CW), "生成 SM2 密钥对").clicked() {
                    let (pk, sk) = gm::gen_sm2_keypair();
                    self.pub_key = pk;
                    self.priv_key = sk;
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
            ui.label(RichText::new("加密 / 摘要").size(15.0).strong().color(theme.fg));
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "您的文本");
                cols[0].add_space(4.0);
                widgets::code_area(&mut cols[0], "gm-enc-text", &mut self.enc_text, true, 5);
                widgets::field_label(&mut cols[1], &theme, "您的密钥 / 口令");
                cols[1].add_space(4.0);
                cols[1].add(TextEdit::singleline(&mut self.enc_key).desired_width(f32::INFINITY).hint_text("SM4 口令 / SM2 公钥"));
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
            });
            ui.add_space(10.0);
            let key_for_enc = if self.enc_algo == EncAlgo::Sm2 {
                self.pub_key.clone()
            } else {
                self.enc_key.clone()
            };
            ui.horizontal(|ui| {
                if widgets::primary_icon(ui, &theme, icons::LOCK, "加密 / 摘要").clicked() {
                    self.enc_out = gm::encrypt(self.enc_algo, &self.enc_text, &key_for_enc)
                        .unwrap_or_else(|e| format!("错误：{e}"));
                }
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制").clicked() {
                    shared.copy(ui.ctx(), self.enc_out.clone());
                }
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
                cols[1].add(TextEdit::singleline(&mut self.dec_key).desired_width(f32::INFINITY).hint_text("SM4 口令 / SM2 私钥"));
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
            });
            ui.add_space(10.0);
            let key_for_dec = if self.dec_algo == DecAlgo::Sm2 {
                self.priv_key.clone()
            } else {
                self.dec_key.clone()
            };
            ui.horizontal(|ui| {
                if widgets::primary_icon(ui, &theme, icons::KEY, "解密").clicked() {
                    self.dec_out = gm::decrypt(self.dec_algo, &self.dec_text, &key_for_dec)
                        .unwrap_or_else(|e| format!("错误：{e}"));
                }
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制").clicked() {
                    shared.copy(ui.ctx(), self.dec_out.clone());
                }
            });
            ui.add_space(8.0);
            widgets::field_label(ui, &theme, "您的解密文本");
            ui.add_space(4.0);
            widgets::code_area(ui, "gm-dec-out", &mut self.dec_out, false, 3);
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
        }
    }
}
