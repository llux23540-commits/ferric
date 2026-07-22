//! JSON 工具视图：单栏就地编辑 + 图标工具条 + 树视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{Frame, Margin, RichText, Stroke, Ui};
use ferric_core::json::{self, Indent};
use serde::{Deserialize, Serialize};

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
    undo: Vec<String>,
    redo: Vec<String>,
}

impl Default for JsonTool {
    fn default() -> Self {
        Self {
            input: demo_json(),
            indent: Indent::Two,
            sort: false,
            ok: true,
            status: "就绪".to_owned(),
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

    /// 切换缩进并**立即**按新缩进重排（内容为合法 JSON 时）。
    fn set_indent(&mut self, indent: Indent) {
        self.indent = indent;
        if let Ok(out) = json::format(&self.input, indent, self.sort) {
            if out != self.input {
                self.undo.push(self.input.clone());
                self.redo.clear();
                self.input = out;
            }
            self.ok = true;
            self.status = "已按新缩进格式化".to_owned();
        }
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
                    if widgets::tb_text_btn(ui, theme, "2", is2, "缩进 2 空格（立即重排）").clicked() {
                        self.set_indent(Indent::Two);
                    }
                    if widgets::tb_text_btn(ui, theme, "4", is4, "缩进 4 空格（立即重排）").clicked() {
                        self.set_indent(Indent::Four);
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::INDENT_INCREASE, is_tab, false, "Tab 缩进（立即重排）").clicked() {
                        self.set_indent(Indent::Tab);
                    }
                    if widgets::tb_icon_btn(ui, theme, icons::ARROW_UP_A_Z, self.sort, false, "键名排序 A→Z").clicked() {
                        self.sort = !self.sort;
                    }
                    widgets::tb_sep(ui, theme);
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

    fn full_bleed(&self) -> bool {
        true // 标题左对齐，编辑区铺满整个内容区
    }

    fn header_actions(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        self.toolbar_row(ui, &theme, shared);
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 实时校验语法：结果直接反映到底部状态条（顶部工具条不再放“校验”按钮）。
        if self.input.trim().is_empty() {
            self.ok = true;
            self.status = "就绪".to_owned();
        } else {
            match json::validate(&self.input) {
                Ok(_) => {
                    self.ok = true;
                    self.status = "JSON 有效".to_owned();
                }
                Err(e) => {
                    self.ok = false;
                    self.status = format!("语法错误：{e}");
                }
            }
        }

        // 底部固定一行状态条（自带顶部分割线），其余空间 100% 交给编辑区。
        egui::Panel::bottom("json-status-bar")
            .exact_size(30.0)
            .frame(Frame::NONE.inner_margin(Margin::symmetric(24, 0)))
            .show_separator_line(false)
            .show(ui, |ui| {
                // 分割线：状态条顶边（横贯整个内容区宽度）
                let rect = ui.max_rect();
                let full = egui::Rangef::new(rect.left() - 24.0, rect.right() + 24.0);
                ui.painter().hline(full, rect.top(), Stroke::new(1.0_f32, theme.border));
                // 整条 30px 高度内垂直居中，图标与文字内联（不嵌套 horizontal，避免对齐偏差）
                ui.horizontal_centered(|ui| {
                    let (glyph, color) = if self.ok {
                        (icons::CIRCLE_CHECK, theme.ok)
                    } else {
                        (icons::CIRCLE_ALERT, theme.danger)
                    };
                    ui.label(icons::text(glyph, 13.0, color));
                    ui.label(RichText::new(&self.status).size(11.5).color(color));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} 字符", self.input.chars().count()))
                                .size(11.0)
                                .family(egui::FontFamily::Monospace)
                                .color(theme.faint),
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::NONE.inner_margin(Margin {
                left: 24,
                right: 24,
                top: 10,
                bottom: 10,
            }))
            .show(ui, |ui| {
                // 单栏：自研代码编辑器（自由编辑 + 语法高亮；后续叠加折叠）。
                let editor_h = ui.available_height();
                widgets::code_editor::code_editor(ui, &theme, "json-in", &mut self.input, editor_h);
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

/// 演示用 JSON：正好 300 个属性、嵌套对象 5 层（3×3×2×3×4 = 216 标量 + 上层键 = 300）。
fn demo_json() -> String {
    use serde_json::{Map, Value};
    let l1 = ["service", "platform", "infra"];
    let l2 = ["core", "edge", "batch"];
    let l3 = ["primary", "backup"];
    let l4 = ["config", "metrics", "state"];
    let l5 = ["id", "enabled", "count", "note"];
    let mut root = Map::new();
    for (gi, g) in l1.iter().enumerate() {
        let mut o2 = Map::new();
        for s in l2 {
            let mut o3 = Map::new();
            for (pi, p) in l3.iter().enumerate() {
                let mut o4 = Map::new();
                for f in l4 {
                    let mut o5 = Map::new();
                    for (i, sn) in l5.iter().enumerate() {
                        let v = match i {
                            0 => Value::String(format!("{g}-{s}-{p}-{f}")),
                            1 => Value::Bool(pi == 0),
                            2 => Value::from((gi as i64 + 1) * 100 + f.len() as i64),
                            _ => Value::Null,
                        };
                        o5.insert((*sn).to_owned(), v);
                    }
                    o4.insert(f.to_owned(), Value::Object(o5));
                }
                o3.insert((*p).to_owned(), Value::Object(o4));
            }
            o2.insert(s.to_owned(), Value::Object(o3));
        }
        root.insert((*g).to_owned(), Value::Object(o2));
    }
    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_roundtrip() {
        let a = JsonTool {
            input: "{\"x\":1}".to_owned(),
            indent: Indent::Tab,
            sort: true,
            ..Default::default()
        };
        let s = a.save_draft().expect("save");

        let mut b = JsonTool::default();
        b.load_draft(&s);
        assert_eq!(b.input, "{\"x\":1}");
        assert_eq!(b.indent, Indent::Tab);
        assert!(b.sort);
    }
}
