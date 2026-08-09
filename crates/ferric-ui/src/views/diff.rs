//! 文本 / 文件对比视图：差异直接高亮在左右两个编辑面板内。

use crate::theme::Theme;
use crate::tool::{Shared, Tool, ToolMeta};
use crate::widgets::{DiffGap, DiffLineStyle};
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

/// 搜索范围侧别：由「最近一次获得焦点的编辑面板」决定。
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

/// 左右滚动同步状态（存 egui 临时数据，一帧一读写）。
///
/// 设计是**一根虚拟滚动条**：`shared` 是唯一事实，每帧把它（按各自上限夹过）
/// 同时强加给两侧 —— 两栏在**同一帧**一起动，不存在「一侧领跑、另一侧下一帧
/// 追赶」的跟随抖动（此前的增量镜像正是这样：平滑滚动的衰减尾巴让右栏被
/// 一小步一小步推着走，看起来就是「左边滚动、右边一直在刷新」）。
///
/// 滚轮在进面板**之前**就被本模块消费并直接作用于 `shared`；滚动条拖动这类
/// 面板内交互产生的偏差，帧末以**增量**折回 `shared`（增量而非绝对：内容短的
/// 一侧滚到底饱和即可，绝不把长侧瞬移回去）。
#[derive(Clone, Copy)]
struct ScrollSync {
    /// 虚拟滚动条位置（共享偏移，夹在 [0, 两侧最大偏移的较大者]）。
    shared: f32,
    /// 上一帧两个面板视口的并集（判断滚轮是否悬停在对比区上）。
    view: egui::Rect,
    /// 上一帧两侧各自的最大偏移（content − viewport）。
    max: [f32; 2],
}

impl Default for ScrollSync {
    fn default() -> Self {
        Self {
            shared: 0.0,
            view: egui::Rect::NOTHING,
            max: [0.0, 0.0],
        }
    }
}

pub struct DiffTool {
    left: String,
    right: String,
    left_name: String,
    right_name: String,
    // —— 搜索状态（会话内临时，不进草稿）——
    /// 搜索条是否展开（Ctrl+F 开 / Esc 关）。
    search_open: bool,
    search_query: String,
    /// 上一帧的搜索词：变化时回到第一个匹配并请求跳转。
    search_prev_query: String,
    /// 当前匹配的全局序号（左侧全部在前、右侧在后）。
    search_current: usize,
    /// 本帧发生打开 / 导航 / 词变化：把当前匹配滚进视口。
    search_nav: bool,
    /// 下一次渲染把焦点交给搜索输入框（Ctrl+F 刚按下）。
    search_focus_req: bool,
    /// 最近一次获得焦点的面板：决定搜索范围（None = 两侧）。
    focus_side: Option<Side>,
    /// 测试探针：最近一帧两侧的实际滚动偏移（[左, 右]），供联动断言读取。
    #[cfg(test)]
    probe_offsets: [f32; 2],
}

impl Default for DiffTool {
    fn default() -> Self {
        Self {
            left: "hello\nworld\nfoo\n".to_owned(),
            right: "hello\nferric\nfoo\nbar\n".to_owned(),
            left_name: String::new(),
            right_name: String::new(),
            search_open: false,
            search_query: String::new(),
            search_prev_query: String::new(),
            search_current: 0,
            search_nav: false,
            search_focus_req: false,
            focus_side: None,
            #[cfg(test)]
            probe_offsets: [0.0, 0.0],
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

    /// 循环前进 / 后退当前匹配，并请求把它滚进视口。
    fn step_match(&mut self, total: usize, forward: bool) {
        if total == 0 {
            return;
        }
        self.search_current = if forward {
            (self.search_current + 1) % total
        } else {
            (self.search_current + total - 1) % total
        };
        self.search_nav = true;
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

/// 帧末结算（纯函数）：把面板内交互（拖滚动条、点击导致的局部滚动）产生的偏差
/// 按**增量**折回共享偏移。
///
/// - `applied[i]` = 本帧渲染前强加给 i 侧的偏移（共享偏移夹到该侧上限）；
/// - `actual[i]` = 渲染后 i 侧的实际偏移，差值即该侧本帧的**本地**滚动量；
/// - 折增量而非取绝对值：内容短的一侧滚到底饱和后，其绝对位置绝不能把长侧
///   拽回去（老的绝对镜像正是「左边滚了等于白滚 / 位置随机跳」的根源）；
/// - 结果夹在 [0, 两侧最大偏移的较大者]。
fn fold_pane_deltas(shared: f32, applied: [f32; 2], actual: [f32; 2], max_both: f32) -> f32 {
    let mut s = shared;
    for i in 0..2 {
        let d = actual[i] - applied[i];
        if d.abs() > 0.5 {
            s += d;
        }
    }
    s.clamp(0.0, max_both.max(0.0))
}
/// 大小写不敏感搜索：逐 char 滑窗比较，不对全文做 to_lowercase——
/// 个别字符小写后 byte 长度会变（如 'İ'），整体转换会让区间映射回原文时错位。
/// 返回各匹配在原文中的 byte 区间，升序且互不重叠。
///
/// 护栏：查询超过 256 字符按无匹配处理（朴素匹配 O(n·m)，粘一大段文本进
/// 查询框不该有能力拖死每一帧）；命中数上限 5000（同代码编辑器）。
fn search_matches(text: &str, query: &str) -> Vec<(usize, usize)> {
    let q: Vec<char> = query.chars().collect();
    if q.is_empty() || q.len() > 256 {
        return Vec::new();
    }
    let idx: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + q.len() <= idx.len() {
        let hit = q
            .iter()
            .zip(&idx[i..])
            .all(|(&qc, &(_, tc))| qc == tc || qc.to_lowercase().eq(tc.to_lowercase()));
        if hit {
            let start = idx[i].0;
            let end = idx.get(i + q.len()).map_or(text.len(), |&(b, _)| b);
            out.push((start, end));
            if out.len() >= 5000 {
                break;
            }
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

/// 由统一 diff 行派生左右两侧面板各自的行样式与缺口标记：
/// 左侧只关心删除（红），右侧只关心新增（绿），未变行透明。
///
/// 缺口的推导：走一遍带 `Tag` 的统一行表，`Insert` 只存在于右侧 —— 它连续出现
/// 几行，左侧就在当前行下标处缺几行；`Delete` 反之。
fn side_styles(
    lines: &[diff::DiffLine],
    theme: &Theme,
) -> (
    Vec<DiffLineStyle>,
    Vec<DiffLineStyle>,
    Vec<DiffGap>,
    Vec<DiffGap>,
) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut lgaps: Vec<DiffGap> = Vec::new();
    let mut rgaps: Vec<DiffGap> = Vec::new();
    // 把连续的 Insert / Delete 合并成一个缺口，而不是一行一个标记
    let push_gap = |gaps: &mut Vec<DiffGap>, at: usize| match gaps.last_mut() {
        Some(g) if g.before_line == at => g.lines += 1,
        _ => gaps.push(DiffGap {
            before_line: at,
            lines: 1,
        }),
    };
    for line in lines {
        match line.tag {
            Tag::Delete => push_gap(&mut rgaps, right.len()),
            Tag::Insert => push_gap(&mut lgaps, left.len()),
            Tag::Equal => {}
        }
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
    (left, right, lgaps, rgaps)
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

    /// 铺满模式：内容区宽度 100% 交给本工具，两个输入框随窗口宽度自动均分。
    fn full_bleed(&self) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        self.handle_drops(ui, shared);

        // Ctrl+F 展开搜索条（打开即请求跳到当前匹配）；Esc 收起。
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.search_open = true;
            self.search_focus_req = true;
            self.search_nav = true;
        }
        if self.search_open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search_open = false;
        }

        let (lines, stats) = diff::line_diff(&self.left, &self.right);
        let (left_styles, right_styles, left_gaps, right_gaps) = side_styles(&lines, &theme);

        // —— 搜索匹配：渲染前算好，搜索条计数与两侧高亮共用一份。——
        // 范围由「最近获得焦点的面板」决定：左 / 右有过焦点就只搜那一侧，否则两侧一起。
        let scope = if self.search_open {
            self.focus_side
        } else {
            None
        };
        // 搜索词变化：回到第一个匹配并请求跳转。
        if self.search_query != self.search_prev_query {
            self.search_prev_query = self.search_query.clone();
            self.search_current = 0;
            self.search_nav = true;
        }
        let (l_matches, r_matches) = if self.search_open && !self.search_query.is_empty() {
            (
                if scope == Some(Side::Right) {
                    Vec::new()
                } else {
                    search_matches(&self.left, &self.search_query)
                },
                if scope == Some(Side::Left) {
                    Vec::new()
                } else {
                    search_matches(&self.right, &self.search_query)
                },
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let total = l_matches.len() + r_matches.len();
        self.search_current = self.search_current.min(total.saturating_sub(1));
        // 当前匹配落在哪一侧（全局序号：左侧全部在前、右侧在后）。
        let cur_left =
            (total > 0 && self.search_current < l_matches.len()).then_some(self.search_current);
        let cur_right = (total > 0 && self.search_current >= l_matches.len())
            .then(|| self.search_current - l_matches.len());

        // 铺满模式下自己负责边距；宽度随窗口变化，两栏始终均分。
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 24,
                right: 24,
                top: 12,
                bottom: 0,
            })
            .show(ui, |ui| {
                let avail_h = ui.available_height();

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

                // 搜索条（统计行下方）：输入 + 范围 + 计数 + 上/下一个 + 关闭。
                // 占掉的高度记下来，从面板高度预算里扣除，保持底边对齐。
                let mut search_extra = 0.0;
                if self.search_open {
                    let bar = ui.horizontal(|ui| {
                        ui.label(icons::text(icons::SEARCH, 13.0, theme.muted));
                        ui.add_space(2.0);
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .id_salt("diff-search")
                                .desired_width(220.0)
                                .hint_text("搜索…"),
                        );
                        if self.search_focus_req {
                            resp.request_focus();
                            self.search_focus_req = false;
                        }
                        // Enter 前进 / Shift+Enter 后退：单行 TextEdit 回车会交出焦点，
                        // 导航完再把焦点夺回来，方便连按。
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let back = ui.input(|i| i.modifiers.shift);
                            self.step_match(total, !back);
                            resp.request_focus();
                        }
                        ui.add_space(8.0);
                        let scope_txt = match scope {
                            Some(Side::Left) => "左侧",
                            Some(Side::Right) => "右侧",
                            None => "两侧",
                        };
                        ui.label(
                            RichText::new(format!("范围：{scope_txt}"))
                                .size(11.5)
                                .color(theme.muted),
                        );
                        ui.add_space(8.0);
                        if !self.search_query.is_empty() {
                            let count = if total == 0 {
                                "0/0".to_owned()
                            } else {
                                format!("{}/{}", self.search_current + 1, total)
                            };
                            ui.label(
                                RichText::new(count)
                                    .size(11.5)
                                    .color(theme.fg_soft)
                                    .monospace(),
                            );
                            ui.add_space(4.0);
                        }
                        if widgets::subtle_button(ui, &theme, None, "上一个").clicked() {
                            self.step_match(total, false);
                        }
                        if widgets::subtle_button(ui, &theme, None, "下一个").clicked() {
                            self.step_match(total, true);
                        }
                        ui.add_space(4.0);
                        if widgets::icon_btn(ui, &theme, icons::X, 24.0).clicked() {
                            self.search_open = false;
                        }
                    });
                    ui.add_space(8.0);
                    search_extra = bar.response.rect.height() + 8.0;
                }

                // 双栏卡片：同高、铺满剩余高度（同 JSON→YAML 页的布局策略）。
                let gutter = 16.0;
                let colw = ((ui.available_width() - gutter) / 2.0).max(200.0);
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
                // 固定开销：统计行、卡片头（含载入按钮）、内边距与各级间距（实测约 108），
                // 底部留和左右一致的 24px 边距，面板高度取精确值。
                let pin_h = (avail_h - 108.0 - 24.0 - search_extra).max(160.0);
                // 行数向下取整（内容 ≤ 视口，避免 1-2px 常驻滚动），
                // 除不尽的余数由 code_area_diff 的 min_inner_h 撑满补齐。
                let rows = (((pin_h - 28.0) / row_h).floor() as usize).max(6);
                let min_inner_h = pin_h - 24.0; // 扣掉编辑框上下内边距
                                                // 载入按钮点击标记：卡片头闭包里不能同时可变借用 self，出布局后统一处理。
                let mut load_left = false;
                let mut load_right = false;

                let left_lines = self.left.lines().count();
                let right_lines = self.right.lines().count();
                let left_name = self.left_name.clone();
                let right_name = self.right_name.clone();

                let l_paint = (!l_matches.is_empty()).then_some(widgets::SearchPaint {
                    matches: &l_matches,
                    current: cur_left,
                });
                let r_paint = (!r_matches.is_empty()).then_some(widgets::SearchPaint {
                    matches: &r_matches,
                    current: cur_right,
                });

                // —— 同步滚动：一根虚拟滚动条。滚轮在进面板之前消费，
                // 本帧就作用于共享偏移，两栏同帧一起动。——
                let sync_id = ui.id().with("diff-scroll-sync");
                let mut sync: ScrollSync = ui.data_mut(|d| d.get_temp(sync_id)).unwrap_or_default();
                let hovering = ui
                    .ctx()
                    .pointer_hover_pos()
                    .is_some_and(|p| sync.view.contains(p));
                if hovering {
                    // 消费方式与 ScrollArea 自己的完全一致：读平滑增量、清零防止
                    // 面板内的 ScrollArea 再吃一遍。
                    let dy = ui.input_mut(|i| {
                        let d = i.smooth_scroll_delta().y;
                        i.smooth_scroll_delta.y = 0.0;
                        d
                    });
                    // 与 ScrollArea 同号约定：offset -= delta。
                    sync.shared -= dy;
                }
                let max_both = sync.max[0].max(sync.max[1]).max(0.0);
                sync.shared = sync.shared.clamp(0.0, max_both);
                // 本帧强加给两侧的偏移（各自夹到自己的上限）。
                let applied = [
                    sync.shared.min(sync.max[0]).max(0.0),
                    sync.shared.min(sync.max[1]).max(0.0),
                ];

                // 两个面板的输出（编辑框响应 + 匹配 y + 实际滚动偏移），出闭包后统一处理。
                let mut l_view: Option<(widgets::DiffAreaOutput, f32, egui::Rect, f32)> = None;
                let mut r_view: Option<(widgets::DiffAreaOutput, f32, egui::Rect, f32)> = None;

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;

                    ui.vertical(|ui| {
                        ui.set_width(colw);
                        widgets::panel(
                            ui,
                            &theme,
                            "左侧 · 原始",
                            |ui| {
                                if widgets::subtle_button(
                                    ui,
                                    &theme,
                                    Some(icons::FOLDER_OPEN),
                                    "载入文件",
                                )
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
                                let out = egui::ScrollArea::vertical()
                                    .id_salt("diff-l-sc")
                                    .min_scrolled_height(pin_h)
                                    .max_height(pin_h)
                                    .auto_shrink([false, false])
                                    .vertical_scroll_offset(applied[0])
                                    .show(ui, |ui| {
                                        widgets::code_area_diff(
                                            ui,
                                            &theme,
                                            "diff-l",
                                            &mut self.left,
                                            rows,
                                            &left_styles,
                                            &left_gaps,
                                            min_inner_h,
                                            l_paint.as_ref(),
                                        )
                                    });
                                let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                                l_view = Some((out.inner, out.state.offset.y, out.inner_rect, max));
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
                                if widgets::subtle_button(
                                    ui,
                                    &theme,
                                    Some(icons::FOLDER_OPEN),
                                    "载入文件",
                                )
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
                                let out = egui::ScrollArea::vertical()
                                    .id_salt("diff-r-sc")
                                    .min_scrolled_height(pin_h)
                                    .max_height(pin_h)
                                    .auto_shrink([false, false])
                                    .vertical_scroll_offset(applied[1])
                                    .show(ui, |ui| {
                                        widgets::code_area_diff(
                                            ui,
                                            &theme,
                                            "diff-r",
                                            &mut self.right,
                                            rows,
                                            &right_styles,
                                            &right_gaps,
                                            min_inner_h,
                                            r_paint.as_ref(),
                                        )
                                    });
                                let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                                r_view = Some((out.inner, out.state.offset.y, out.inner_rect, max));
                            },
                        );
                    });
                });

                // panel 的 body 闭包一定会执行，这里必然有值。
                let (l_area, l_scroll, l_rect, l_max) = l_view.expect("左面板已渲染");
                let (r_area, r_scroll, r_rect, r_max) = r_view.expect("右面板已渲染");

                // —— 焦点范围跟踪：记住最近获得焦点的面板；点击空白让焦点彻底落空时
                // 回到「两侧」。点搜索框 / 按钮时焦点仍有归属，不会误改范围。——
                if l_area.response.gained_focus() {
                    self.focus_side = Some(Side::Left);
                }
                if r_area.response.gained_focus() {
                    self.focus_side = Some(Side::Right);
                }
                if ui.input(|i| i.pointer.any_pressed())
                    && ui.ctx().memory(|m| m.focused()).is_none()
                {
                    self.focus_side = None;
                }

                // —— 帧末结算：滚动条拖动等面板内交互产生的偏差，按**增量**折回
                // 共享偏移（纯函数 fold_pane_deltas，契约见其单测）。——
                let folded =
                    fold_pane_deltas(sync.shared, applied, [l_scroll, r_scroll], l_max.max(r_max));
                // 有本地交互（拖滚动条）→ 下一帧把另一侧带过去，需要再画一帧。
                if (folded - sync.shared).abs() > 0.5 {
                    ui.ctx().request_repaint();
                }
                sync.shared = folded;
                sync.max = [l_max, r_max];
                sync.view = l_rect.union(r_rect);
                #[cfg(test)]
                {
                    self.probe_offsets = [l_scroll, r_scroll];
                }

                // —— 搜索跳转：导航帧从面板拿到当前匹配 y，直接落到共享偏移，
                // 下一帧两栏一起滚到视口约 1/3 处。——
                if self.search_nav {
                    let y = if cur_left.is_some() {
                        l_area.current_match_y
                    } else {
                        r_area.current_match_y
                    };
                    if let Some(y) = y {
                        sync.shared = (y - pin_h / 3.0).max(0.0);
                        ui.ctx().request_repaint();
                    }
                    self.search_nav = false;
                }
                ui.data_mut(|d| d.insert_temp(sync_id, sync));

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

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户报的问题：**「左侧多了一部分，右侧就看不出来差异在哪」**。
    ///
    /// 两栏是各自独立的编辑框，一侧被删掉几行，另一侧的后续内容只是整体上移，
    /// 对面那栏没有任何痕迹。这条断言「缺口」被算了出来 —— 位置与行数都要对，
    /// 它是缺口标记能画对的前提。
    #[test]
    fn a_run_of_deletions_becomes_one_gap_on_the_other_side() {
        let theme = Theme::light();
        // 左边多出 DEL-1..3；右边多出 ADD-1..2
        let left = "alpha\nbravo\nDEL-1\nDEL-2\nDEL-3\ncharlie\ndelta\n";
        let right = "alpha\nbravo\ncharlie\nADD-1\nADD-2\ndelta\n";
        let (lines, _) = diff::line_diff(left, right);
        let (_, _, lgaps, rgaps) = side_styles(&lines, &theme);

        // 右侧在「charlie 之前」缺 3 行（左边那三条 DEL）
        assert_eq!(
            rgaps.len(),
            1,
            "连续删除应当合并成一个缺口：{:?}",
            rgaps
                .iter()
                .map(|g| (g.before_line, g.lines))
                .collect::<Vec<_>>()
        );
        assert_eq!(rgaps[0].lines, 3, "右侧缺口行数不对");
        assert_eq!(
            rgaps[0].before_line, 2,
            "右侧缺口应落在第 3 行（charlie）之前"
        );

        // 左侧在「delta 之前」缺 2 行（右边那两条 ADD）
        assert_eq!(lgaps.len(), 1, "连续新增应当合并成一个缺口");
        assert_eq!(lgaps[0].lines, 2, "左侧缺口行数不对");
        assert_eq!(
            lgaps[0].before_line, 6,
            "左侧缺口应落在第 7 行（delta）之前"
        );
    }

    /// 两边完全一样时不该冒出任何缺口标记 —— 否则界面上会凭空多出横线。
    #[test]
    fn identical_sides_have_no_gaps() {
        let theme = Theme::light();
        let (lines, _) = diff::line_diff("a\nb\nc\n", "a\nb\nc\n");
        let (_, _, lgaps, rgaps) = side_styles(&lines, &theme);
        assert!(lgaps.is_empty() && rgaps.is_empty());
    }

    /// 命中区间必须是**原文 byte 区间**且落在 char 边界上 —— 含 CJK 时
    /// 直接决定高亮切分与跳转不会 panic。
    #[test]
    fn search_matches_returns_byte_ranges_on_char_boundaries() {
        let text = "标记 Needle 与 needle 两处";
        let hits = search_matches(text, "NEEDLE");
        assert_eq!(hits.len(), 2);
        for (s, e) in hits {
            assert!(text.is_char_boundary(s) && text.is_char_boundary(e));
            assert!(text[s..e].eq_ignore_ascii_case("needle"));
        }
    }

    /// 护栏：超长查询按无匹配处理；命中数不超过上限。
    #[test]
    fn search_matches_guards_pathological_input() {
        assert!(search_matches("aaaa", &"a".repeat(300)).is_empty());
        let text = "x".repeat(20_000);
        assert_eq!(search_matches(&text, "x").len(), 5000);
    }

    /// 空查询：无匹配（搜索条刚打开时不该把全文都圈起来）。
    #[test]
    fn search_matches_empty_query_is_empty() {
        assert!(search_matches("anything", "").is_empty());
    }

    /// 静止帧：两侧实际偏移与强加值一致 → 共享偏移原地不动，
    /// 不产生任何「追赶」——这是「右边一直在刷新」不再复发的前提。
    #[test]
    fn idle_frames_leave_shared_untouched() {
        assert_eq!(
            fold_pane_deltas(2000.0, [2000.0, 145.0], [2000.0, 145.0], 4000.0),
            2000.0
        );
    }

    /// 短侧饱和不回拽：共享偏移 2000、右侧被夹到上限 145 渲染 —— 右侧实际
    /// 停在 145 与强加值相同，**不算**本地滚动，长侧位置分毫不动。
    /// 老的绝对镜像在这里会把左侧瞬移回 145（「左边滚了等于白滚」）。
    #[test]
    fn saturated_short_side_never_drags_shared_back() {
        assert_eq!(
            fold_pane_deltas(2000.0, [2000.0, 145.0], [2000.0, 145.0], 2800.0),
            2000.0
        );
    }

    /// 拖动右侧滚动条回滚 10px：折回共享偏移的是**增量** −10，
    /// 而不是右侧的绝对位置 —— 左侧只回退 10px，不发生大跳变。
    #[test]
    fn scrollbar_drag_folds_as_delta() {
        let s = fold_pane_deltas(2000.0, [2000.0, 145.0], [2000.0, 135.0], 2800.0);
        assert_eq!(s, 1990.0);
        assert!(
            (s - 2000.0).abs() < 50.0,
            "共享偏移出现大跳变（{s}），说明退回了绝对镜像"
        );
    }

    /// 共享偏移始终夹在 [0, 两侧上限的较大者]：越界拖动不会把状态拖飞。
    #[test]
    fn shared_offset_is_clamped_to_the_longer_side() {
        assert_eq!(
            fold_pane_deltas(100.0, [100.0, 100.0], [5000.0, 100.0], 2800.0),
            2800.0
        );
        assert_eq!(fold_pane_deltas(5.0, [5.0, 5.0], [0.0, 5.0], 2800.0), 0.0);
        assert_eq!(fold_pane_deltas(-3.0, [0.0, 0.0], [0.0, 0.0], 2800.0), 0.0);
    }

    /// 端到端（真实输入管线）：滚轮悬停在左栏上滚动，两栏必须**同一帧**一起动；
    /// 停手并等平滑滚动衰减结束后，偏移完全静止 —— 「左边滚动、右边一直在
    /// 刷新」的跟随式抖动不允许复发。
    #[test]
    fn wheel_scrolls_both_panes_in_lockstep_and_settles() {
        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::light();
        theme.apply(&ctx);
        crate::fonts::install_fonts(&ctx); // 图标字体（Lucide）在裸 Context 里不存在
        let mut shared = crate::tool::Shared::new(theme);
        let mut tool = DiffTool {
            left: (0..300).map(|i| format!("line {i} left\n")).collect(),
            right: (0..60).map(|i| format!("line {i} right\n")).collect(),
            ..Default::default()
        };

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let at = egui::pos2(350.0, 400.0); // 左栏内部
        let mut t = 0.0_f64;
        let mut frame =
            |tool: &mut DiffTool, shared: &mut crate::tool::Shared, events: Vec<egui::Event>| {
                t += 0.05;
                let out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        time: Some(t),
                        events,
                        ..Default::default()
                    },
                    |ui| tool.ui(ui, shared),
                );
                let _ = out;
                tool.probe_offsets
            };

        // 预热两帧：注册面板视口（悬停判定用上一帧的并集）。
        frame(&mut tool, &mut shared, vec![]);
        frame(&mut tool, &mut shared, vec![egui::Event::PointerMoved(at)]);

        // 连续滚轮：每一帧里两栏都必须一起动（右栏饱和前）。
        let mut prev = tool.probe_offsets;
        let mut lockstep_checked = 0;
        for _ in 0..8 {
            let now = frame(
                &mut tool,
                &mut shared,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -3.0),
                    modifiers: Default::default(),
                    phase: egui::TouchPhase::Move,
                }],
            );
            let (dl, dr) = (now[0] - prev[0], now[1] - prev[1]);
            // 右栏尚未到底时：左动右必动，且发生在同一帧。
            if dl > 0.5 && dr > 0.5 {
                lockstep_checked += 1;
            }
            prev = now;
        }
        assert!(
            prev[0] > 10.0,
            "滚轮没有滚动左栏（{prev:?}）—— 悬停消费路径断了"
        );
        assert!(
            lockstep_checked >= 2,
            "没有观察到任何一帧两栏同帧联动（右栏饱和前应当步调一致）"
        );

        // 停手：跑够衰减期后，偏移必须完全静止（无跟随式追赶）。
        for _ in 0..80 {
            frame(&mut tool, &mut shared, vec![]);
        }
        let a = frame(&mut tool, &mut shared, vec![]);
        let b = frame(&mut tool, &mut shared, vec![]);
        assert_eq!(a, b, "静置后偏移仍在变化 —— 「右边一直在刷新」复发");
    }
}
