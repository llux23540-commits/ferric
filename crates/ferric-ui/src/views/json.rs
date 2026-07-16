//! JSON 工具视图：单栏就地编辑 + 图标工具条 + 树视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{
    vec2, Align2, Color32, FontId, Frame, Margin, RichText, Rounding, ScrollArea, Sense, Stroke,
    TextWrapMode, Ui,
};
use ferric_core::json::{self, Indent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

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
    /// 折叠视图中已收起的块路径。
    collapsed: HashSet<String>,
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
            tree: false,
            collapsed: HashSet::new(),
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
                self.collapsed.clear();
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
        self.collapsed.clear();
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
                self.collapsed.clear();
            }
            self.ok = true;
            self.status = "已按新缩进格式化".to_owned();
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.input, prev));
            self.collapsed.clear();
            self.status = "已撤销".to_owned();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.input, next));
            self.collapsed.clear();
            self.status = "已重做".to_owned();
        }
    }

    fn toolbar_row(&mut self, ui: &mut Ui, theme: &crate::theme::Theme, shared: &mut Shared) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 2.0);
            let (indent, sort) = (self.indent, self.sort);
            if widgets::tb_icon_btn(ui, theme, icons::ALIGN_LEFT, false, false, "格式化 / 美化")
                .clicked()
            {
                self.run_op(|s| json::format(s, indent, sort), "已格式化 · JSON 有效");
            }
            if widgets::tb_icon_btn(ui, theme, icons::WRAP_TEXT, false, false, "压缩为单行")
                .clicked()
            {
                self.run_op(json::minify, "已压缩为单行");
            }
            if widgets::tb_icon_btn(ui, theme, icons::CIRCLE_CHECK, false, false, "校验语法")
                .clicked()
            {
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
            if widgets::tb_icon_btn(ui, theme, icons::QUOTE, false, false, "转义为 JSON 字符串")
                .clicked()
            {
                let out = json::escape(&self.input);
                self.replace(out, "已转义为 JSON 字符串");
            }
            if widgets::tb_icon_btn(ui, theme, icons::CODE, false, false, "去除转义").clicked()
            {
                self.run_op(json::unescape, "已去除转义");
            }
            if widgets::tb_icon_btn(ui, theme, icons::ERASER, false, false, "去除全部空白")
                .clicked()
            {
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
            if widgets::tb_text_btn(ui, theme, "2", is2, "缩进 2 空格（立即重排）").clicked()
            {
                self.set_indent(Indent::Two);
            }
            if widgets::tb_text_btn(ui, theme, "4", is4, "缩进 4 空格（立即重排）").clicked()
            {
                self.set_indent(Indent::Four);
            }
            if widgets::tb_icon_btn(
                ui,
                theme,
                icons::INDENT_INCREASE,
                is_tab,
                false,
                "Tab 缩进（立即重排）",
            )
            .clicked()
            {
                self.set_indent(Indent::Tab);
            }
            if widgets::tb_icon_btn(
                ui,
                theme,
                icons::ARROW_UP_A_Z,
                self.sort,
                false,
                "键名排序 A→Z",
            )
            .clicked()
            {
                self.sort = !self.sort;
            }
            widgets::tb_sep(ui, theme);
            if widgets::tb_icon_btn(
                ui,
                theme,
                icons::LIST_TREE,
                self.tree,
                false,
                "折叠视图（点箭头收起/展开）",
            )
            .clicked()
            {
                self.tree = !self.tree;
            }
            if widgets::tb_icon_btn(ui, theme, icons::COPY, false, false, "复制").clicked() {
                let out = self.input.clone();
                shared.copy(ui.ctx(), out);
            }
            if widgets::tb_icon_btn(ui, theme, icons::FILE_DOWN, false, false, "下载 .json")
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("data.json")
                    .add_filter("JSON", &["json"])
                    .save_file()
                {
                    let _ = std::fs::write(path, &self.input);
                    shared.toast("已保存");
                }
            }
            if widgets::tb_icon_btn(ui, theme, icons::TRASH_2, false, false, "清空输入").clicked()
            {
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

        // 底部固定一行状态条（自带顶部分割线），其余空间 100% 交给编辑区。
        egui::TopBottomPanel::bottom("json-status-bar")
            .exact_height(30.0)
            .frame(Frame::none().inner_margin(Margin::symmetric(24.0, 0.0)))
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                // 分割线：状态条顶边（横贯整个内容区宽度）
                let rect = ui.max_rect();
                let full = egui::Rangef::new(rect.left() - 24.0, rect.right() + 24.0);
                ui.painter()
                    .hline(full, rect.top(), Stroke::new(1.0_f32, theme.border));
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
            .frame(Frame::none().inner_margin(Margin {
                left: 24.0,
                right: 24.0,
                top: 10.0,
                bottom: 10.0,
            }))
            .show_inside(ui, |ui| {
                let editor_h = ui.available_height();
                if self.tree {
                    match serde_json::from_str::<Value>(&self.input) {
                        Ok(v) => {
                            fold_view(ui, &theme, &v, &mut self.collapsed, editor_h);
                        }
                        Err(e) => {
                            self.ok = false;
                            ui.colored_label(theme.danger, format!("无法解析为 JSON：{e}"));
                        }
                    }
                } else {
                    widgets::code_area_seamless(ui, &theme, "json-in", &mut self.input, editor_h);
                }
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

// ---------- 折叠视图（只读、行号 + ▸/▾ 折叠箭头、铺满高度）----------

/// 一段文本的语义类别，用于着色。
#[derive(Clone, Copy)]
enum Col {
    Punct,
    Key,
    Str,
    Num,
    Bool,
    Null,
}

fn seg_color(c: Col, theme: &crate::theme::Theme) -> Color32 {
    match c {
        Col::Key => theme.accent_strong,
        Col::Str => theme.ok,
        Col::Num => {
            if theme.dark {
                Color32::from_rgb(0xe0, 0xb0, 0x62)
            } else {
                Color32::from_rgb(0xb0, 0x6f, 0x00)
            }
        }
        Col::Bool => theme.danger,
        Col::Null | Col::Punct => theme.muted,
    }
}

/// 折叠视图的一「行」。
struct FoldRow {
    indent: usize,
    /// `Some(是否已收起)` 表示该行是一个可折叠块的起始行；`None` 为普通行。
    fold: Option<bool>,
    /// 可折叠块的稳定路径（用于记忆收起状态）。
    path: String,
    segs: Vec<(String, Col)>,
}

/// 递归把 JSON 展开成「显示行」，遇到已收起的块只输出一行摘要。
fn build_rows(
    value: &Value,
    key: Option<&str>,
    path: String,
    indent: usize,
    trailing: &str,
    collapsed: &HashSet<String>,
    out: &mut Vec<FoldRow>,
) {
    let mut segs: Vec<(String, Col)> = Vec::new();
    if let Some(k) = key {
        segs.push((format!("\"{k}\""), Col::Key));
        segs.push((": ".to_owned(), Col::Punct));
    }
    match value {
        Value::Object(map) if !map.is_empty() => {
            if collapsed.contains(&path) {
                segs.push((format!("{{ … }}{trailing}"), Col::Punct));
                out.push(FoldRow {
                    indent,
                    fold: Some(true),
                    path,
                    segs,
                });
            } else {
                segs.push(("{".to_owned(), Col::Punct));
                out.push(FoldRow {
                    indent,
                    fold: Some(false),
                    path: path.clone(),
                    segs,
                });
                let n = map.len();
                for (i, (k, v)) in map.iter().enumerate() {
                    let tc = if i + 1 < n { "," } else { "" };
                    build_rows(
                        v,
                        Some(k),
                        format!("{path}/{k}"),
                        indent + 1,
                        tc,
                        collapsed,
                        out,
                    );
                }
                out.push(FoldRow {
                    indent,
                    fold: None,
                    path: String::new(),
                    segs: vec![(format!("}}{trailing}"), Col::Punct)],
                });
            }
        }
        Value::Array(arr) if !arr.is_empty() => {
            if collapsed.contains(&path) {
                segs.push((format!("[ … ]{trailing}"), Col::Punct));
                out.push(FoldRow {
                    indent,
                    fold: Some(true),
                    path,
                    segs,
                });
            } else {
                segs.push(("[".to_owned(), Col::Punct));
                out.push(FoldRow {
                    indent,
                    fold: Some(false),
                    path: path.clone(),
                    segs,
                });
                let n = arr.len();
                for (i, v) in arr.iter().enumerate() {
                    let tc = if i + 1 < n { "," } else { "" };
                    build_rows(
                        v,
                        None,
                        format!("{path}/{i}"),
                        indent + 1,
                        tc,
                        collapsed,
                        out,
                    );
                }
                out.push(FoldRow {
                    indent,
                    fold: None,
                    path: String::new(),
                    segs: vec![(format!("]{trailing}"), Col::Punct)],
                });
            }
        }
        Value::Object(_) => {
            segs.push((format!("{{}}{trailing}"), Col::Punct));
            out.push(FoldRow {
                indent,
                fold: None,
                path,
                segs,
            });
        }
        Value::Array(_) => {
            segs.push((format!("[]{trailing}"), Col::Punct));
            out.push(FoldRow {
                indent,
                fold: None,
                path,
                segs,
            });
        }
        Value::String(s) => {
            segs.push((format!("\"{s}\"{trailing}"), Col::Str));
            out.push(FoldRow {
                indent,
                fold: None,
                path,
                segs,
            });
        }
        Value::Number(n) => {
            segs.push((format!("{n}{trailing}"), Col::Num));
            out.push(FoldRow {
                indent,
                fold: None,
                path,
                segs,
            });
        }
        Value::Bool(b) => {
            segs.push((format!("{b}{trailing}"), Col::Bool));
            out.push(FoldRow {
                indent,
                fold: None,
                path,
                segs,
            });
        }
        Value::Null => {
            segs.push((format!("null{trailing}"), Col::Null));
            out.push(FoldRow {
                indent,
                fold: None,
                path,
                segs,
            });
        }
    }
}

/// 渲染只读折叠视图，铺满 `height`。点行首箭头收起/展开对应块。
fn fold_view(
    ui: &mut Ui,
    theme: &crate::theme::Theme,
    value: &Value,
    collapsed: &mut HashSet<String>,
    height: f32,
) {
    let mut rows: Vec<FoldRow> = Vec::new();
    build_rows(value, None, "$".to_owned(), 0, "", collapsed, &mut rows);
    let digits = rows.len().to_string().len().max(2);

    let fill = ui.visuals().extreme_bg_color;
    let border = ui.visuals().window_stroke;
    let inner_h = (height - 24.0).max(60.0);
    let font = egui::TextStyle::Monospace.resolve(ui.style());

    let mut toggle: Option<String> = None;
    Frame::none()
        .fill(fill)
        .stroke(border)
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.set_height(inner_h);
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
            ScrollArea::vertical()
                .id_salt("json-fold-sc")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, row) in rows.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            // 行号
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{:>digits$} ", i + 1))
                                        .font(font.clone())
                                        .color(theme.faint),
                                )
                                .wrap_mode(TextWrapMode::Extend)
                                .selectable(false),
                            );
                            // 折叠箭头（固定列，可点）
                            let (arect, aresp) =
                                ui.allocate_exact_size(vec2(16.0, row_h), Sense::click());
                            if let Some(is_col) = row.fold {
                                let ch = if is_col {
                                    icons::CHEVRON_RIGHT
                                } else {
                                    icons::CHEVRON_DOWN
                                };
                                let acol = if aresp.hovered() {
                                    theme.fg
                                } else {
                                    theme.muted
                                };
                                ui.painter().text(
                                    arect.center(),
                                    Align2::CENTER_CENTER,
                                    ch,
                                    FontId::new(13.0, icons::family()),
                                    acol,
                                );
                                if aresp.clicked() {
                                    toggle = Some(row.path.clone());
                                }
                            }
                            // 缩进
                            ui.add_space(row.indent as f32 * 14.0);
                            // 文本段（按类别着色，可选中/复制）
                            for (t, c) in &row.segs {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(t)
                                            .font(font.clone())
                                            .color(seg_color(*c, theme)),
                                    )
                                    .wrap_mode(TextWrapMode::Extend)
                                    .selectable(true),
                                );
                            }
                        });
                    }
                });
        });

    if let Some(p) = toggle {
        if !collapsed.remove(&p) {
            collapsed.insert(p);
        }
    }
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
