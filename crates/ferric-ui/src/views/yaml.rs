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
                if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制 YAML")
                    .on_hover_text("复制选中内容（未选中时复制全部，保留原文）")
                    .clicked()
                {
                    let out = widgets::selected_or_all(ui.ctx(), "yaml-out", &self.output);
                    shared.copy(ui.ctx(), out);
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
        // 固定开销：工具条、卡片头、内边距、状态行与各级间距（约 128），
        // 另留一行文字的底部间距，与对比页一致。
        let box_h = (shared.content_height - 128.0 - row_h).max(160.0);
        let rows = (((box_h - 28.0) / row_h).floor() as usize).max(6);
        // 视口高度按行数精确反推（编辑框内边距 24 + 描边余量），
        // 保证内容 ≤ 视口，否则会差出 1-2px 常驻可滚动状态。
        let pin_h = rows as f32 * row_h + 28.0;
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
                            .min_scrolled_height(pin_h)
                            .max_height(pin_h)
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
                            .min_scrolled_height(pin_h)
                            .max_height(pin_h)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunUiExt;

    #[test]
    fn yaml_copy_button_prefers_selection_or_copies_all() {
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let mut shared = Shared::new(crate::theme::Theme::dark());
        let mut tool = YamlTool {
            input: "{\"name\":\"测试𐐀\",\"items\":[1,2,3]}".to_owned(),
            ..Default::default()
        };
        tool.convert();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));

        let mut copy_btn_pos = None;
        let _ = ctx.run_ui_cleared(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                tool.ui(ui, &mut shared);
            },
        );
        for shape in &ctx
            .run_ui_cleared(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    tool.ui(ui, &mut shared);
                },
            )
            .shapes
        {
            if let egui::Shape::Text(text) = &shape.shape {
                if text.galley.text().contains("复制 YAML") {
                    copy_btn_pos = Some(text.pos + text.galley.size() / 2.0);
                }
            }
        }
        let copy_btn_pos = copy_btn_pos.expect("找到复制 YAML 按钮");

        // 1. 无选区：点击复制按钮复制全文
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
        assert_eq!(full_copied, Some(tool.output.clone()));

        // 2. 在 yaml-out 中选取包含中文的键值对 "name: 测试𐐀"
        let selected_part = "name: 测试𐐀";
        let start_byte = tool.output.find(selected_part).unwrap();
        let start_char = tool.output[..start_byte].chars().count();
        let end_char = start_char + selected_part.chars().count();

        let mut state = egui::text_edit::TextEditState::load(&ctx, egui::Id::new("yaml-out"))
            .expect("存在 yaml-out 状态");
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start_char),
                egui::text::CCursor::new(end_char),
            )));
        state.store(&ctx, egui::Id::new("yaml-out"));

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

        // 再次点击复制按钮：应复制选区 "name: 测试𐐀"
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
