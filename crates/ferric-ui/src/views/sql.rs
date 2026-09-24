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
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制")
                    .on_hover_text("复制选中内容（未选中时复制全部，保留原文）")
                    .clicked()
                {
                    let out = widgets::selected_or_all(ui.ctx(), "sql-in", &self.input);
                    shared.copy(ui.ctx(), out);
                }
            });
        });
        ui.add_space(10.0);

        // 编辑器（带 SQL 头）
        Frame::NONE
            .fill(theme.code_bg)
            .corner_radius(egui::CornerRadius::same(12))
            .show(ui, |ui| {
                Frame::NONE
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new("SQL")
                                .size(11.0)
                                .family(egui::FontFamily::Monospace)
                                .color(theme.faint),
                        );
                    });
                Frame::NONE.inner_margin(Margin::same(4)).show(ui, |ui| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunUiExt;

    #[test]
    fn sql_copy_button_prefers_selection_or_copies_all() {
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let mut shared = Shared::new(crate::theme::Theme::dark());
        let mut tool = SqlTool {
            input: "SELECT 用户名, 年龄 FROM 用户表 WHERE 年龄 > 18;".to_owned(),
            ..Default::default()
        };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));

        let mut copy_btn_pos = None;
        let out = ctx.run_ui_cleared(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                tool.ui(ui, &mut shared);
            },
        );
        for shape in &out.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                if text.galley.text().contains("复制") {
                    copy_btn_pos = Some(text.pos + text.galley.size() / 2.0);
                }
            }
        }
        let copy_btn_pos = copy_btn_pos.expect("找到复制按钮位置");

        // 模拟点击复制按钮
        let out = ctx.run_ui_cleared(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![
                    egui::Event::PointerMoved(copy_btn_pos),
                    egui::Event::PointerButton {
                        pos: copy_btn_pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
                time: Some(0.1),
                ..Default::default()
            },
            |ui| {
                tool.ui(ui, &mut shared);
            },
        );
        let out2 = ctx.run_ui_cleared(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![egui::Event::PointerButton {
                    pos: copy_btn_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                time: Some(0.2),
                ..Default::default()
            },
            |ui| {
                tool.ui(ui, &mut shared);
            },
        );
        let mut full_copied = None;
        for cmd in out
            .platform_output
            .commands
            .into_iter()
            .chain(out2.platform_output.commands)
        {
            if let egui::output::OutputCommand::CopyText(s) = cmd {
                full_copied = Some(s);
            }
        }
        assert_eq!(full_copied, Some(tool.input.clone()));

        // 2. 设置选区为 "WHERE 年龄 > 18"
        let selected_part = "WHERE 年龄 > 18";
        let start_char = tool.input.find(selected_part).unwrap();
        let start_char_idx = tool.input[..start_char].chars().count();
        let end_char_idx = start_char_idx + selected_part.chars().count();

        let mut state = egui::text_edit::TextEditState::load(&ctx, egui::Id::new("sql-in"))
            .expect("存在 sql-in 状态");
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start_char_idx),
                egui::text::CCursor::new(end_char_idx),
            )));
        state.store(&ctx, egui::Id::new("sql-in"));

        // 渲染一帧建立选区记忆
        let _ = ctx.run_ui_cleared(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                tool.ui(ui, &mut shared);
            },
        );

        // 再次点击复制按钮：应复制选区 "WHERE 年龄 > 18"
        let out = ctx.run_ui_cleared(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![
                    egui::Event::PointerMoved(copy_btn_pos),
                    egui::Event::PointerButton {
                        pos: copy_btn_pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
                time: Some(0.3),
                ..Default::default()
            },
            |ui| {
                tool.ui(ui, &mut shared);
            },
        );
        let out2 = ctx.run_ui_cleared(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![egui::Event::PointerButton {
                    pos: copy_btn_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                time: Some(0.4),
                ..Default::default()
            },
            |ui| {
                tool.ui(ui, &mut shared);
            },
        );
        let mut part_copied = None;
        for cmd in out
            .platform_output
            .commands
            .into_iter()
            .chain(out2.platform_output.commands)
        {
            if let egui::output::OutputCommand::CopyText(s) = cmd {
                part_copied = Some(s);
            }
        }
        assert_eq!(part_copied, Some(selected_part.to_owned()));
    }
}
