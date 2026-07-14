//! RSA 密钥对生成视图（后台线程生成）。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{RichText, Ui};
use ferric_core::rsa;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::Receiver;

#[derive(Serialize, Deserialize)]
struct RsaDraft {
    bits: i64,
}

pub struct RsaTool {
    bits: i64,
    pub_pem: String,
    priv_pem: String,
    status: String,
    ok: bool,
    busy: bool,
    rx: Option<Receiver<Result<(String, String), String>>>,
}

impl Default for RsaTool {
    fn default() -> Self {
        let mut t = Self {
            bits: 2048,
            pub_pem: String::new(),
            priv_pem: String::new(),
            status: "就绪".to_owned(),
            ok: true,
            busy: false,
            rx: None,
        };
        t.regen();
        t
    }
}

impl RsaTool {
    fn regen(&mut self) {
        let bits = self.bits.clamp(256, 4096) as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(rsa::generate(bits));
        });
        self.rx = Some(rx);
        self.busy = true;
        self.status = "生成中…".to_owned();
    }

    fn poll(&mut self, ui: &Ui) {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Ok((p, s))) => {
                    self.pub_pem = p;
                    self.priv_pem = s;
                    self.status = "已生成".to_owned();
                    self.ok = true;
                    self.busy = false;
                    self.rx = None;
                }
                Ok(Err(e)) => {
                    self.status = format!("生成失败：{e}");
                    self.ok = false;
                    self.busy = false;
                    self.rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.ctx().request_repaint(); // 继续轮询
                }
                Err(_) => {
                    self.status = "生成线程意外中断".to_owned();
                    self.ok = false;
                    self.busy = false;
                    self.rx = None;
                }
            }
        }
    }
}

impl Tool for RsaTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "rsa",
            name: "RSA 密钥对",
            group: "生成",
            desc: "生成新的随机 RSA 私钥和公钥 pem 证书。",
            icon: icons::KEY,
            keywords: &["rsa", "key", "密钥", "pem", "公钥", "私钥"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        self.poll(ui);

        // 位数 + 刷新
        ui.horizontal(|ui| {
            widgets::field_label(ui, &theme, "位数：");
            ui.add_space(4.0);
            widgets::num_field(ui, &theme, &mut self.bits, 256, 4096, 256);
            ui.add_space(10.0);
            let enabled = !self.busy;
            ui.add_enabled_ui(enabled, |ui| {
                if widgets::subtle_button(ui, &theme, Some(icons::REFRESH_CW), "刷新密钥对").clicked() {
                    self.regen();
                }
            });
            ui.add_space(6.0);
            if self.busy {
                ui.label(RichText::new(&self.status).size(12.0).color(theme.muted));
            } else {
                widgets::status_line(ui, &theme, self.ok, &self.status);
            }
        });
        ui.add_space(16.0);

        // 公钥
        widgets::field_label(ui, &theme, "公钥");
        ui.add_space(6.0);
        if widgets::code_box(ui, &theme, "rsa-pub", &self.pub_pem, 6) {
            shared.copy(ui.ctx(), self.pub_pem.clone());
        }
        ui.add_space(14.0);

        // 私钥
        widgets::field_label(ui, &theme, "私钥");
        ui.add_space(6.0);
        if widgets::code_box(ui, &theme, "rsa-priv", &self.priv_pem, 10) {
            shared.copy(ui.ctx(), self.priv_pem.clone());
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&RsaDraft { bits: self.bits }).ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<RsaDraft>(data) {
            self.bits = d.bits.clamp(256, 4096);
        }
    }
}
