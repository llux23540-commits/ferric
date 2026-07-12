//! JSON 工具视图：单栏就地编辑 + 图标工具条 + 树视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{CollapsingHeader, Color32, Frame, Margin, RichText, ScrollArea, Stroke, Ui};
use ferric_core::json::{self, Indent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
struct JsonDraft {
    input: String,
    indent: Indent,
    sort: bool,
}

pub struct JsonTool {
    input: String,
    indent: Indent,
    sort: bool,
    ok: bool,
    status: String,
    tree: bool,
    undo: Vec<String>,
    redo: Vec<String>,
}

impl Default for JsonTool {
    fn default() -> Self {
        Self {
            input: "{\"name\":\"ferric\",\"tags\":[\"json\",\"diff\"],\"ok\":true}".to_owned(),
            indent: Indent::Two,
            sort: false,
            ok: true,
            status: "就绪".to_owned(),
            tree: false,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl JsonTool {
    fn run_op(&mut self, f: impl FnOnce(&str) -> Result<String, String>, done: &str) {
        match f(&self.input) {
            Ok(out) => {
                self.undo.push(self.input.clone());
                self.redo.clear();
                self.input = out;
                self.ok = true;
                self.status = done.to_owned();
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("解析失败：{e}");
            }
        }
    }

    fn replace(&mut self, out: String, done: &str) {
        self.undo.push(self.input.clone());
        self.redo.clear();
        self.input = out;
        self.ok = true;
        self.status = done.to_owned();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.input, prev));
            self.status = "已撤销".to_owned();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.input, next));
            self.status = "已重做".to_owned();
        }
    }

    fn toolbar_row(&mut self, ui: &mut Ui, theme: &crate::theme::Theme, shared: &mut Shared) {
        ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(3.0, 2.0);
                    let (indent, sort) = (self.indent, self.sort);
                    if widgets::tb_icon_btn(ui, theme, icons::ALIGN_LEFT, false, false, "格式化 / 美化").clicked() {
                        self.run_op(|s| json::format(s, indent, sort), "已格式化 · JSON 有效");
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::WRAP_TEXT, false, false, "压缩为单行").clicked() {
                        self.run_op(json::minify, "已压缩为单行");
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::CIRCLE_CHECK, false, false, "校验语法").clicked() {
                        match json::validate(&self.input) {
                            Ok(_) => {
                                self.ok = true;
                                self.status = "JSON 有效".to_owned();
                            }
                            Err(e) => {
                                self.ok = false;
                                self.status = format!("解析失败：{e}");
                            }
                        }
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::QUOTE, false, false, "转义为 JSON 字符串").clicked() {
                        let out = json::escape(&self.input);
                        self.replace(out, "已转义为 JSON 字符串");
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::CODE, false, false, "去除转义").clicked() {
                        self.run_op(json::unescape, "已去除转义");
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::ERASER, false, false, "去除全部空白").clicked() {
                        self.run_op(json::minify, "已去除全部空白");
                    }
                    widgets::tb_sep(ui, theme);
                    if widgets::tb_icon_btn(ui, theme, icons::UNDO_2, false, false, "撤销").clicked() {
                        self.undo();
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::REDO_2, false, false, "重做").clicked() {
                        self.redo();
                    }
                    widgets::tb_sep(ui, theme);
                    // 缩进：2 / 4 / Tab（图标式按钮，取代药丸段控）
                    let (is2, is4, is_tab) = match self.indent {
                        Indent::Two => (true, false, false),
                        Indent::Four => (false, true, false),
                        Indent::Tab => (false, false, true),
                    };
                    if widgets::tb_text_btn(ui, theme, "2", is2, "缩进 2 空格").clicked() {
                        self.indent = Indent::Two;
                    }
                    if widgets::tb_text_btn(ui, theme, "4", is4, "缩进 4 空格").clicked() {
                        self.indent = Indent::Four;
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::INDENT_INCREASE, is_tab, false, "Tab 缩进").clicked() {
                        self.indent = Indent::Tab;
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::HASH, self.sort, false, "键名排序 A→Z").clicked() {
                        self.sort = !self.sort;
                    }
                    widgets::tb_sep(ui, theme);
                    if widgets::tb_icon_btn(ui, theme, icons::LIST_TREE, self.tree, false, "树视图 / 折叠").clicked() {
                        self.tree = !self.tree;
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::COPY, false, false, "复制").clicked() {
                        let out = self.input.clone();
                        shared.copy(ui.ctx(), out);
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::FILE_DOWN, false, false, "下载 .json").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("data.json")
                            .add_filter("JSON", &["json"])
                            .save_file()
                        {
                            let _ = std::fs::write(path, &self.input);
                            shared.toast("已保存");
                        }
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::TRASH_2, false, false, "清空输入").clicked() {
                        self.replace(String::new(), "已清空");
                    }
        });
    }
}

impl Tool for JsonTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "json",
            name: "JSON 工具",
            group: "JSON",
            desc: "在此粘贴或修改 JSON —— 美化 / 压缩 / 校验 / 转义 / 键名排序等操作都在上方工具条中，随手可用。",
            icon: crate::icons::BRACES,
            keywords: &["json", "format", "beautify", "minify", "美化", "格式化", "压缩"],
        }
    }

    fn show_desc(&self) -> bool {
        false // 工具条在顶栏、就地编辑，无需描述行
    }

    fn header_actions(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        self.toolbar_row(ui, &theme, shared);
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        ui.add_space(4.0);

        // 编辑区铺满剩余高度：预留 4 上距 + 4 空隙 + 状态行(~20) + 极小余量，底部只留一行描述。
        let editor_h = (shared.content_height - 40.0).max(160.0);

        if self.tree {
            match serde_json::from_str::<Value>(&self.input) {
                Ok(v) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("树视图 · 点节点可折叠").size(12.0).color(theme.muted));
                    });
                    ui.add_space(6.0);
                    Frame::none()
                        .fill(theme.code_bg)
                        .stroke(Stroke::new(1.0, theme.border))
                        .rounding(egui::Rounding::same(10.0))
                        .inner_margin(Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ScrollArea::vertical().max_height((editor_h - 24.0).max(160.0)).auto_shrink([false, false]).show(
                                ui,
                                |ui| {
                                    render_node(ui, &theme, None, &v, true);
                                },
                            );
                        });
                }
                Err(e) => {
                    self.ok = false;
                    ui.colored_label(theme.danger, format!("无法解析为 JSON：{e}"));
                }
            }
        } else {
            widgets::code_area_fill(ui, "json-in", &mut self.input, editor_h);
        }

        ui.add_space(5.0);
        ui.horizontal(|ui| {
            widgets::status_line(ui, &theme, self.ok, &self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} 字符", self.input.chars().count()))
                        .size(11.0)
                        .family(egui::FontFamily::Monospace)
                        .color(theme.faint),
                );
            });
        });
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&JsonDraft {
            input: self.input.clone(),
            indent: self.indent,
            sort: self.sort,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<JsonDraft>(data) {
            self.input = d.input;
            self.indent = d.indent;
            self.sort = d.sort;
        }
    }
}

/// 递归渲染 JSON 树节点。
fn render_node(ui: &mut Ui, theme: &crate::theme::Theme, key: Option<&str>, val: &Value, root: bool) {
    let key_col = theme.accent_strong;
    let str_col = theme.ok;
    let num_col = if theme.dark {
        Color32::from_rgb(0xe0, 0xb0, 0x62)
    } else {
        Color32::from_rgb(0xb0, 0x6f, 0x00)
    };
    let bool_col = theme.danger;

    let prefix = |ui: &mut Ui| {
        if let Some(k) = key {
            ui.label(RichText::new(format!("\"{k}\"")).monospace().size(13.0).color(key_col));
            ui.label(RichText::new(": ").monospace().size(13.0).color(theme.muted));
        }
    };

    match val {
        Value::Object(map) => {
            let title = match key {
                Some(k) => format!("\"{k}\"  {{{} 键}}", map.len()),
                None => format!("{{{} 键}}", map.len()),
            };
            CollapsingHeader::new(RichText::new(title).monospace().size(13.0).color(key_col))
                .id_salt(format!("obj-{:p}-{}", map, key.unwrap_or("root")))
                .default_open(root)
                .show(ui, |ui| {
                    for (k, v) in map {
                        render_node(ui, theme, Some(k), v, false);
                    }
                });
        }
        Value::Array(arr) => {
            let title = match key {
                Some(k) => format!("\"{k}\"  [{} 项]", arr.len()),
                None => format!("[{} 项]", arr.len()),
            };
            CollapsingHeader::new(RichText::new(title).monospace().size(13.0).color(key_col))
                .id_salt(format!("arr-{:p}-{}", arr, key.unwrap_or("root")))
                .default_open(root)
                .show(ui, |ui| {
                    for (i, v) in arr.iter().enumerate() {
                        render_node(ui, theme, Some(&i.to_string()), v, false);
                    }
                });
        }
        Value::String(s) => {
            ui.horizontal(|ui| {
                prefix(ui);
                ui.label(RichText::new(format!("\"{s}\"")).monospace().size(13.0).color(str_col));
            });
        }
        Value::Number(n) => {
            ui.horizontal(|ui| {
                prefix(ui);
                ui.label(RichText::new(n.to_string()).monospace().size(13.0).color(num_col));
            });
        }
        Value::Bool(b) => {
            ui.horizontal(|ui| {
                prefix(ui);
                ui.label(RichText::new(b.to_string()).monospace().size(13.0).color(bool_col));
            });
        }
        Value::Null => {
            ui.horizontal(|ui| {
                prefix(ui);
                ui.label(RichText::new("null").monospace().size(13.0).italics().color(theme.muted));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_roundtrip() {
        let mut a = JsonTool::default();
        a.input = "{\"x\":1}".to_owned();
        a.indent = Indent::Tab;
        a.sort = true;
        let s = a.save_draft().expect("save");

        let mut b = JsonTool::default();
        b.load_draft(&s);
        assert_eq!(b.input, "{\"x\":1}");
        assert_eq!(b.indent, Indent::Tab);
        assert!(b.sort);
    }
}
