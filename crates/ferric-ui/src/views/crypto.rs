//! 加密 / 解密文本视图（AES / TripleDES / Rabbit / RC4，OpenSSL 盐格式）。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{ComboBox, TextEdit, Ui};
use ferric_core::crypto::{self, Algo};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct CryptoDraft {
    enc_text: String,
    dec_text: String,
    enc_algo: Algo,
    dec_algo: Algo,
}

pub struct CryptoTool {
    enc_text: String,
    enc_key: String,
    enc_out: String,
    enc_algo: Algo,
    enc_ok: bool,
    enc_status: String,
    dec_text: String,
    dec_key: String,
    dec_out: String,
    dec_algo: Algo,
    dec_ok: bool,
    dec_status: String,
}

impl Default for CryptoTool {
    fn default() -> Self {
        Self {
            enc_text: String::new(),
            enc_key: String::new(),
            enc_out: String::new(),
            enc_algo: Algo::Aes,
            enc_ok: true,
            enc_status: "就绪".to_owned(),
            dec_text: String::new(),
            dec_key: String::new(),
            dec_out: String::new(),
            dec_algo: Algo::Aes,
            dec_ok: true,
            dec_status: "就绪".to_owned(),
        }
    }
}

fn algo_combo(ui: &mut Ui, id: &str, algo: &mut Algo) {
    ComboBox::from_id_salt(id)
        .selected_text(algo.label())
        .show_ui(ui, |ui| {
            for a in Algo::ALL {
                ui.selectable_value(algo, a, a.label());
            }
        });
}

impl Tool for CryptoTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "crypto",
            name: "加密 / 解密文本",
            group: "加密",
            desc: "使用加密算法（AES、TripleDES、Rabbit、RC4）加密和解密文本明文。",
            icon: icons::LOCK,
            keywords: &["crypto", "aes", "加密", "解密", "encrypt"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 加密卡片
        widgets::card(ui, &theme, |ui| {
            ui.label(egui::RichText::new("加密").size(15.0).strong().color(theme.fg));
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "您的文本");
                cols[0].add_space(4.0);
                widgets::code_area(&mut cols[0], "enc-text", &mut self.enc_text, true, 5);

                widgets::field_label(&mut cols[1], &theme, "您的密钥");
                cols[1].add_space(4.0);
                cols[1].add(TextEdit::singleline(&mut self.enc_key).desired_width(f32::INFINITY).hint_text("密钥"));
                cols[1].add_space(8.0);
                widgets::field_label(&mut cols[1], &theme, "加密算法");
                cols[1].add_space(4.0);
                algo_combo(&mut cols[1], "enc-algo", &mut self.enc_algo);
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary_icon(ui, &theme, icons::LOCK, "加密").clicked() {
                    match crypto::encrypt(self.enc_algo, &self.enc_text, &self.enc_key) {
                        Ok(ct) => {
                            self.enc_out = ct;
                            self.enc_ok = true;
                            self.enc_status = "已加密".to_owned();
                        }
                        Err(e) => {
                            self.enc_out.clear();
                            self.enc_ok = false;
                            self.enc_status = format!("加密失败：{e}");
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
            widgets::field_label(ui, &theme, "您的加密文本");
            ui.add_space(4.0);
            widgets::code_area(ui, "enc-out", &mut self.enc_out, false, 3);
        });
        ui.add_space(14.0);

        // 解密卡片
        widgets::card(ui, &theme, |ui| {
            ui.label(egui::RichText::new("解密").size(15.0).strong().color(theme.fg));
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "您的加密文本");
                cols[0].add_space(4.0);
                widgets::code_area(&mut cols[0], "dec-text", &mut self.dec_text, true, 5);

                widgets::field_label(&mut cols[1], &theme, "您的密钥");
                cols[1].add_space(4.0);
                cols[1].add(TextEdit::singleline(&mut self.dec_key).desired_width(f32::INFINITY).hint_text("密钥"));
                cols[1].add_space(8.0);
                widgets::field_label(&mut cols[1], &theme, "加密算法");
                cols[1].add_space(4.0);
                algo_combo(&mut cols[1], "dec-algo", &mut self.dec_algo);
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary_icon(ui, &theme, icons::KEY, "解密").clicked() {
                    match crypto::decrypt(self.dec_algo, &self.dec_text, &self.dec_key) {
                        Ok(pt) => {
                            self.dec_out = pt;
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
            widgets::code_area(ui, "dec-out", &mut self.dec_out, false, 3);
        });
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&CryptoDraft {
            enc_text: self.enc_text.clone(),
            dec_text: self.dec_text.clone(),
            enc_algo: self.enc_algo,
            dec_algo: self.dec_algo,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<CryptoDraft>(data) {
            self.enc_text = d.enc_text;
            self.dec_text = d.dec_text;
            self.enc_algo = d.enc_algo;
            self.dec_algo = d.dec_algo;
        }
    }
}
