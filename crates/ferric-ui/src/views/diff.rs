//! 文本 / 文件对比视图：差异直接高亮在左右两个编辑面板内。

use crate::theme::Theme;
use crate::tool::{Shared, Tool, ToolMeta};
use crate::widgets::DiffLineStyle;
use crate::{icons, widgets};
use egui::{Color32, RichText, Ui};
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
    fn handle_drops(&mut self, ui: &Ui, shared: &mut Shared) {
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

/// 由统一 diff 行派生左右两侧面板各自的行样式：
/// 左侧只关心删除（红），右侧只关心新增（绿），未变行透明。
fn side_styles(
    lines: &[diff::DiffLine],
    theme: &Theme,
) -> (Vec<DiffLineStyle>, Vec<DiffLineStyle>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for line in lines {
        match line.tag {
            Tag::Equal => {
                let plain = DiffLineStyle {
                    bg: Color32::TRANSPARENT,
                    mark: Color32::TRANSPARENT,
                    segs: line.segs.clone(),
                };
                left.push(plain.clone());
                right.push(plain);
            }
            Tag::Delete => left.push(DiffLineStyle {
                bg: theme.del_bg,
                mark: theme.del_mark,
                segs: line.segs.clone(),
            }),
            Tag::Insert => right.push(DiffLineStyle {
                bg: theme.add_bg,
                mark: theme.add_mark,
                segs: line.segs.clone(),
            }),
        }
    }
    (left, right)
}

impl Tool for DiffTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "diff",
            name: "文本 / 文件对比",
            group: "对比",
            desc: "逐行比较两段文本或文件，差异直接高亮在两侧编辑框：左侧标删除，右侧标新增，可载入或拖入文件。",
            icon: icons::GIT_COMPARE,
            keywords: &["diff", "compare", "对比", "比较", "差异"],
        }
    }

    fn show_desc(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        self.handle_drops(ui, shared);

        let (lines, stats) = diff::line_diff(&self.left, &self.right);
        let (left_styles, right_styles) = side_styles(&lines, &theme);

        // 顶部统计行
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("+{} 新增", stats.added))
                    .color(theme.ok)
                    .size(13.0)
                    .strong(),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!("−{} 删除", stats.removed))
                    .color(theme.danger)
                    .size(13.0)
                    .strong(),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!("={} 未变", stats.unchanged))
                    .color(theme.muted)
                    .size(13.0),
            );
        });
        ui.add_space(10.0);

        // 双栏卡片：同高、铺满剩余高度（同 JSON→YAML 页的布局策略）。
        let gutter = 16.0;
        let colw = ((ui.available_width() - gutter) / 2.0).max(200.0);
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        // 固定开销：统计行、卡片头（含载入按钮）、内边距与各级间距（无底部状态行）
        let box_h = (shared.content_height - 180.0).max(160.0);
        let rows = (((box_h - 24.0) / row_h).floor() as usize).max(6);
        // 载入按钮点击标记：卡片头闭包里不能同时可变借用 self，出布局后统一处理。
        let mut load_left = false;
        let mut load_right = false;

        let left_lines = self.left.lines().count();
        let right_lines = self.right.lines().count();
        let left_name = self.left_name.clone();
        let right_name = self.right_name.clone();

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            ui.vertical(|ui| {
                ui.set_width(colw);
                widgets::panel(
                    ui,
                    &theme,
                    "左侧 · 原始",
                    |ui| {
                        if widgets::subtle_button(ui, &theme, Some(icons::FOLDER_OPEN), "载入文件")
                            .clicked()
                        {
                            load_left = true;
                        }
                        if !left_name.is_empty() {
                            ui.label(
                                RichText::new(&left_name)
                                    .size(11.0)
                                    .color(theme.muted)
                                    .monospace(),
                            );
                            ui.add_space(8.0);
                        }
                        ui.label(
                            RichText::new(format!("{left_lines} 行"))
                                .size(11.0)
                                .color(theme.faint),
                        );
                    },
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("diff-l-sc")
                            .min_scrolled_height(box_h)
                            .max_height(box_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                widgets::code_area_diff(
                                    ui,
                                    &theme,
                                    "diff-l",
                                    &mut self.left,
                                    rows,
                                    &left_styles,
                                );
                            });
                    },
                );
            });

            ui.add_space(gutter);

            ui.vertical(|ui| {
                ui.set_width(colw);
                widgets::panel(
                    ui,
                    &theme,
                    "右侧 · 修改后",
                    |ui| {
                        if widgets::subtle_button(ui, &theme, Some(icons::FOLDER_OPEN), "载入文件")
                            .clicked()
                        {
                            load_right = true;
                        }
                        if !right_name.is_empty() {
                            ui.label(
                                RichText::new(&right_name)
                                    .size(11.0)
                                    .color(theme.muted)
                                    .monospace(),
                            );
                            ui.add_space(8.0);
                        }
                        ui.label(
                            RichText::new(format!("{right_lines} 行"))
                                .size(11.0)
                                .color(theme.faint),
                        );
                    },
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("diff-r-sc")
                            .min_scrolled_height(box_h)
                            .max_height(box_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                widgets::code_area_diff(
                                    ui,
                                    &theme,
                                    "diff-r",
                                    &mut self.right,
                                    rows,
                                    &right_styles,
                                );
                            });
                    },
                );
            });
        });

        // 卡片头里点了「载入文件」：出布局后统一弹窗读取
        if load_left {
            match pick_file() {
                Ok(Some((n, t))) => {
                    self.left = t;
                    self.left_name = n;
                }
                Ok(None) => {}
                Err(e) => shared.toast(format!("读取文件失败：{e}")),
            }
        }
        if load_right {
            match pick_file() {
                Ok(Some((n, t))) => {
                    self.right = t;
                    self.right_name = n;
                }
                Ok(None) => {}
                Err(e) => shared.toast(format!("读取文件失败：{e}")),
            }
        }
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
