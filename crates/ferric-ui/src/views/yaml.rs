//! JSON → YAML 转换视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{RichText, Ui};
use ferric_core::yaml;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct YamlDraft {
    input: String,
}

pub struct YamlTool {
    input: String,
    output: String,
    ok: bool,
    status: String,
}

impl Default for YamlTool {
    fn default() -> Self {
        let mut t = Self {
            input: "{\"name\":\"ferric\",\"tags\":[\"json\",\"yaml\"],\"ok\":true}".to_owned(),
            output: String::new(),
            ok: true,
            status: String::new(),
        };
        t.convert();
        t
    }
}

impl YamlTool {
    fn convert(&mut self) {
        match yaml::json_to_yaml(&self.input) {
            Ok(y) => {
                self.output = y;
                self.ok = true;
                self.status = "JSON 有效".to_owned();
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("解析失败：{e}");
            }
        }
    }
}

impl Tool for YamlTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "yaml",
            name: "JSON → YAML",
            group: "转换",
            desc: "简单地将 JSON 转换为 YAML —— 左侧输入 JSON，右侧实时输出 YAML。",
            icon: icons::LIST_CHECKS,
            keywords: &["yaml", "json", "转换", "convert"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        ui.columns(2, |cols| {
            // 左：JSON
            cols[0].horizontal(|ui| {
                widgets::field_label(ui, &theme, "JSON");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (glyph, col) = if self.ok {
                        (icons::CIRCLE_CHECK, theme.ok)
                    } else {
                        (icons::CIRCLE_ALERT, theme.danger)
                    };
                    ui.label(RichText::new(&self.status).size(11.5).color(col));
                    ui.label(icons::text(glyph, 13.0, col));
                });
            });
            cols[0].add_space(4.0);
            if widgets::code_area(&mut cols[0], "yaml-in", &mut self.input, true, 16).changed() {
                self.convert();
            }

            // 右：YAML
            cols[1].horizontal(|ui| {
                widgets::field_label(ui, &theme, "YAML");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制").clicked() {
                        shared.copy(ui.ctx(), self.output.clone());
                    }
                });
            });
            cols[1].add_space(4.0);
            widgets::code_area(&mut cols[1], "yaml-out", &mut self.output, false, 16);
        });
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&YamlDraft {
            input: self.input.clone(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<YamlDraft>(data) {
            self.input = d.input;
            self.convert();
        }
    }
}
