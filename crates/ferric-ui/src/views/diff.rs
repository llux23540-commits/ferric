//! 文本 / 文件对比视图。

use crate::theme::Theme;
use crate::{icons, widgets};
use egui::{Color32, Frame, Margin, RichText, ScrollArea, Stroke, Ui};
use ferric_core::diff::{self, Tag};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct DiffDraft {
    left: String,
    right: String,
    #[serde(default)]
    left_name: String,
    #[serde(default)]
    right_name: String,
}

pub struct DiffTool {
    left: String,
    right: String,
    left_name: String,
    right_name: String,
}

impl Default for DiffTool {
    fn default() -> Self {
        Self {
            left: "hello\nworld\nfoo\n".to_owned(),
            right: "hello\nferric\nfoo\nbar\n".to_owned(),
            left_name: String::new(),
            right_name: String::new(),
        }
    }
}

impl DiffTool {
    /// 处理拖入的文件：按指针水平位置决定落到左 / 右侧。
    fn handle_drops(&mut self, ui: &Ui, shared: &mut crate::tool::Shared) {
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // 以内容区（而非整个窗口）的中线分左右，避免侧栏偏移导致误判。
        let center_x = ui.max_rect().center().x;
        let pointer_x = ui
            .ctx()
            .input(|i| i.pointer.hover_pos().map(|p| p.x))
            .unwrap_or(center_x);
        for file in dropped {
            if let Some(path) = &file.path {
                match std::fs::read_to_string(path) {
                    Ok(text) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if pointer_x < center_x {
                            self.left = text;
                            self.left_name = name;
                        } else {
                            self.right = text;
                            self.right_name = name;
                        }
                    }
                    Err(e) => shared.toast(format!("读取文件失败：{e}")),
                }
            }
        }
    }
}

/// 选择并读取文件。`Ok(None)` 表示用户取消；读取失败返回 `Err(原因)`。
fn pick_file() -> Result<Option<(String, String)>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter(
            "文本",
            &[
                "txt", "json", "md", "csv", "log", "xml", "yml", "yaml", "js", "ts", "css", "html",
                "sql", "rs", "toml",
            ],
        )
        .pick_file()
    else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Some((name, text)))
}

fn row_style(tag: Tag, theme: &Theme) -> (Color32, Color32, Color32, &'static str) {
    match tag {
        Tag::Equal => (Color32::TRANSPARENT, theme.muted, Color32::TRANSPARENT, " "),
        Tag::Insert => (theme.add_bg, theme.ok, theme.add_mark, "+"),
        Tag::Delete => (theme.del_bg, theme.danger, theme.del_mark, "-"),
    }
}

impl DiffTool {
    fn column(
        ui: &mut Ui,
        theme: &Theme,
        title: &str,
        name: &mut String,
        text: &mut String,
        id: &str,
        shared: &mut crate::tool::Shared,
    ) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(12.5).color(theme.fg_soft));
            ui.label(
                RichText::new(format!("{} 行", text.lines().count()))
                    .size(11.0)
                    .color(theme.faint),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::subtle_button(ui, theme, Some(icons::FOLDER_OPEN), "载入文件").clicked() {
                    match pick_file() {
                        Ok(Some((n, t))) => {
                            *text = t;
                            *name = n;
                        }
                        Ok(None) => {}
                        Err(e) => shared.toast(format!("读取文件失败：{e}")),
                    }
                }
                if !name.is_empty() {
                    ui.label(RichText::new(name.as_str()).size(11.0).color(theme.muted).monospace());
                }
            });
        });
        ui.add_space(4.0);
        widgets::code_area(ui, id, text, true, 10);
    }
}

impl crate::tool::Tool for DiffTool {
    fn meta(&self) -> crate::tool::ToolMeta {
        crate::tool::ToolMeta {
            id: "diff",
            name: "文本 / 文件对比",
            group: "对比",
            desc: "逐行比较两段文本或文件，改动行做字符级高亮。",
            icon: crate::icons::GIT_COMPARE,
            keywords: &["diff", "compare", "对比", "比较", "差异"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut crate::tool::Shared) {
        let theme = shared.theme;
        self.handle_drops(ui, shared);

        ui.label(
            RichText::new("逐行比较两段内容 —— 每一侧都可以粘贴文本、载入文件，或直接把文件拖进输入框。")
                .size(12.5)
                .color(theme.muted),
        );
        ui.add_space(10.0);

        // 双栏编辑器
        ui.columns(2, |cols| {
            Self::column(&mut cols[0], &theme, "左侧 · 原始", &mut self.left_name, &mut self.left, "diff-l", shared);
            Self::column(&mut cols[1], &theme, "右侧 · 修改后", &mut self.right_name, &mut self.right, "diff-r", shared);
        });

        ui.add_space(14.0);
        let (lines, stats) = diff::line_diff(&self.left, &self.right);

        // 结果块
        widgets::card(ui, &theme, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("+{} 新增", stats.added)).color(theme.ok).size(13.0).strong());
                ui.add_space(10.0);
                ui.label(RichText::new(format!("−{} 删除", stats.removed)).color(theme.danger).size(13.0).strong());
                ui.add_space(10.0);
                ui.label(RichText::new(format!("={} 未变", stats.unchanged)).color(theme.muted).size(13.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    legend_swatch(ui, &theme, theme.del_mark, "删除");
                    legend_swatch(ui, &theme, theme.add_mark, "新增");
                });
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            ScrollArea::vertical()
                .max_height(360.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for line in &lines {
                        let (bg, sign_col, mark, sign) = row_style(line.tag, &theme);
                        Frame::none()
                            .fill(bg)
                            .inner_margin(Margin::symmetric(6.0, 1.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    let ln = |n: Option<usize>| {
                                        n.map(|v| v.to_string()).unwrap_or_default()
                                    };
                                    ui.label(
                                        RichText::new(format!(
                                            "{:>4} {:>4} ",
                                            ln(line.left_no),
                                            ln(line.right_no)
                                        ))
                                        .monospace()
                                        .size(12.0)
                                        .color(theme.faint),
                                    );
                                    ui.label(
                                        RichText::new(format!("{sign} "))
                                            .monospace()
                                            .size(12.5)
                                            .color(sign_col),
                                    );
                                    for seg in &line.segs {
                                        let mut rt = RichText::new(&seg.text)
                                            .monospace()
                                            .size(12.5)
                                            .color(theme.fg);
                                        if seg.emph {
                                            rt = rt.background_color(mark);
                                        }
                                        ui.label(rt);
                                    }
                                });
                            });
                    }
                });
        });
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&DiffDraft {
            left: self.left.clone(),
            right: self.right.clone(),
            left_name: self.left_name.clone(),
            right_name: self.right_name.clone(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<DiffDraft>(data) {
            self.left = d.left;
            self.right = d.right;
            self.left_name = d.left_name;
            self.right_name = d.right_name;
        }
    }
}

fn legend_swatch(ui: &mut Ui, theme: &Theme, color: Color32, label: &str) {
    ui.label(RichText::new(label).size(11.5).color(theme.muted));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 12.0), egui::Sense::hover());
    ui.painter().rect(
        rect,
        egui::Rounding::same(3.0),
        color,
        Stroke::new(1.0_f32, theme.border_2),
    );
    ui.add_space(8.0);
}
