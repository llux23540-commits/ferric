//! SQL 格式化视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{Frame, Margin, RichText, Ui};
use ferric_core::sql;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SqlDraft {
    input: String,
    uppercase: bool,
}

pub struct SqlTool {
    input: String,
    uppercase: bool,
    status: String,
}

impl Default for SqlTool {
    fn default() -> Self {
        Self {
            input: "select id,name,email from users where age>18 order by name".to_owned(),
            uppercase: true,
            status: "就绪".to_owned(),
        }
    }
}

impl Tool for SqlTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "sql",
            name: "SQL 格式化",
            group: "SQL",
            desc: "美化 / 压缩 SQL，关键字换行缩进，可选关键字大写。",
            icon: icons::DATABASE,
            keywords: &["sql", "format", "格式化", "美化"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 工具条
        ui.horizontal_wrapped(|ui| {
            if widgets::primary_icon(ui, &theme, icons::CHECK, "格式化").clicked() {
                self.input = sql::format(&self.input, self.uppercase);
                self.status = "已格式化".to_owned();
            }
            if widgets::ghost_button(ui, &theme, "压缩为单行").clicked() {
                self.input = sql::minify(&self.input);
                self.status = "已压缩为单行".to_owned();
            }
            if widgets::pill_toggle(ui, &theme, self.uppercase, "关键字大写") {
                self.uppercase = !self.uppercase;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::subtle_button(ui, &theme, Some(icons::TRASH_2), "清空").clicked() {
                    self.input.clear();
                    self.status = "已清空".to_owned();
                }
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制").clicked() {
                    shared.copy(ui.ctx(), self.input.clone());
                }
            });
        });
        ui.add_space(10.0);

        // 编辑器（带 SQL 头）
        Frame::none()
            .fill(theme.code_bg)
            .rounding(egui::Rounding::same(12.0))
            .show(ui, |ui| {
                Frame::none()
                    .inner_margin(Margin::symmetric(14.0, 8.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new("SQL")
                                .size(11.0)
                                .family(egui::FontFamily::Monospace)
                                .color(theme.faint),
                        );
                    });
                Frame::none()
                    .inner_margin(Margin::same(4.0))
                    .show(ui, |ui| {
                        widgets::code_area(ui, "sql-in", &mut self.input, true, 16);
                    });
            });

        ui.add_space(8.0);
        widgets::status_line(ui, &theme, true, &self.status);
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&SqlDraft {
            input: self.input.clone(),
            uppercase: self.uppercase,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<SqlDraft>(data) {
            self.input = d.input;
            self.uppercase = d.uppercase;
        }
    }
}
