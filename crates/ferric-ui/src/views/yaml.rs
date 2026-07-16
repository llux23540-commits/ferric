//! JSON → YAML 转换视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{vec2, Align, Layout, RichText, Ui};
use ferric_core::yaml;
use serde::{Deserialize, Serialize};

const SAMPLE: &str = "{\"name\":\"ferric\",\"tags\":[\"json\",\"yaml\"],\"ok\":true}";

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
            input: SAMPLE.to_owned(),
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
        if self.input.trim().is_empty() {
            self.output.clear();
            self.ok = true;
            self.status = "就绪 —— 输入 JSON 后实时转换".to_owned();
            return;
        }
        match yaml::json_to_yaml(&self.input) {
            Ok(y) => {
                self.output = y;
                self.ok = true;
                self.status = "JSON 有效 · 已实时转换".to_owned();
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

        // 工具条
        ui.horizontal_wrapped(|ui| {
            if widgets::subtle_button(ui, &theme, Some(icons::QUOTE), "载入示例").clicked() {
                self.input = SAMPLE.to_owned();
                self.convert();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::subtle_button(ui, &theme, Some(icons::TRASH_2), "清空").clicked() {
                    self.input.clear();
                    self.convert();
                }
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制 YAML").clicked() {
                    shared.copy(ui.ctx(), self.output.clone());
                }
            });
        });
        ui.add_space(10.0);

        // 双栏卡片 + 中间转换方向箭头。
        // 高度铺满：用外壳测得的内容区总高，扣掉工具条 / 卡片头 / 状态行等固定开销，
        // 剩余全部给编辑框；左右两框同高，超长内容在框内滚动，状态行始终可见。
        let gutter = 30.0;
        let colw = ((ui.available_width() - gutter) / 2.0).max(200.0);
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        // 固定开销：工具条、卡片头、内边距、状态行与各级间距（实测约 200，留少量余量）
        let box_h = (shared.content_height - 200.0).max(160.0);
        let rows = (((box_h - 24.0) / row_h).floor() as usize).max(6);
        // 箭头对齐编辑区垂直中线：卡片头高 + 半个编辑框
        let arrow_y = 30.0 + 4.0 + box_h * 0.5;

        let in_lines = self.input.lines().count();
        let out_lines = self.output.lines().count();
        let mut input_changed = false;

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            ui.vertical(|ui| {
                ui.set_width(colw);
                widgets::panel(
                    ui,
                    &theme,
                    "JSON",
                    |ui| {
                        ui.label(
                            RichText::new(format!("{in_lines} 行"))
                                .size(11.0)
                                .color(theme.faint),
                        );
                    },
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("yaml-in-sc")
                            .min_scrolled_height(box_h)
                            .max_height(box_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                input_changed =
                                    widgets::code_area(ui, "yaml-in", &mut self.input, true, rows)
                                        .changed();
                            });
                    },
                );
            });

            ui.allocate_ui_with_layout(
                vec2(gutter, arrow_y * 2.0),
                Layout::top_down(Align::Center),
                |ui| {
                    ui.add_space(arrow_y);
                    ui.label(icons::text(icons::CHEVRON_RIGHT, 18.0, theme.faint));
                },
            );

            ui.vertical(|ui| {
                ui.set_width(colw);
                widgets::panel(
                    ui,
                    &theme,
                    "YAML",
                    |ui| {
                        ui.label(
                            RichText::new(format!("{out_lines} 行"))
                                .size(11.0)
                                .color(theme.faint),
                        );
                    },
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("yaml-out-sc")
                            .min_scrolled_height(box_h)
                            .max_height(box_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                widgets::code_area(ui, "yaml-out", &mut self.output, false, rows);
                            });
                    },
                );
            });
        });
        if input_changed {
            self.convert();
        }

        ui.add_space(8.0);
        widgets::status_line(ui, &theme, self.ok, &self.status);
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
