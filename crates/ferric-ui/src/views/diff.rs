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

/// 搜索范围侧别：由「最近一次获得焦点的编辑面板」决定。
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

/// 左右滚动同步状态（存 egui 临时数据，一帧一读写）。
#[derive(Clone, Copy, Default)]
struct ScrollSync {
    /// 上一帧两侧 ScrollArea 的 y 偏移（[左, 右]）。
    prev: [f32; 2],
    /// 本帧要给某侧加的**增量**（镜像用户在对侧的滚动量）。
    ///
    /// 必须是增量而不是绝对偏移：两侧内容长度往往不同，短的一侧早早滚到底被
    /// 夹住，若把它的绝对位置镜像回长的一侧，长侧会被**瞬移**回短侧的上限 ——
    /// 实测表现就是「左边滚了等于白滚 / 位置随机跳」。增量镜像下，被夹住的一侧
    /// 只是原地饱和，谁也不会拽着谁跳。
    nudge: [Option<f32>; 2],
    /// 本帧要**跳转**到的绝对偏移（仅搜索定位用：两侧同一目标行，允许绝对值）。
    jump: [Option<f32>; 2],
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

/// 同步滚动的镜像决策（纯函数）：哪侧被用户滚动了，就把**增量**镜像给对侧。
///
/// - `forced[i]` = i 侧本帧的偏移是程序强加的（镜像/搜索跳转），其变化不算用户滚动，
///   否则镜像回来的偏移会被当成新滚动，左右互相追着抖；
/// - 两侧同帧都动时以变化量大的一侧为准；
/// - 返回增量而非绝对偏移：短的一侧滚到底被夹住后，绝对镜像会把长的一侧
///   **瞬移**回短侧上限（实测就是「左边滚了等于白滚 / 位置随机跳」的根源），
///   增量镜像下被夹侧只是原地饱和，谁也不拽着谁跳。
fn mirror_scroll(prev: [f32; 2], now: [f32; 2], forced: [bool; 2]) -> [Option<f32>; 2] {
    let dl = if forced[0] { 0.0 } else { now[0] - prev[0] };
    let dr = if forced[1] { 0.0 } else { now[1] - prev[1] };
    let mut out = [None, None];
    if dl.abs() >= dr.abs() {
        if dl.abs() > 0.5 {
            out[1] = Some(dl);
        }
    } else if dr.abs() > 0.5 {
        out[0] = Some(dr);
    }
    out
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
        let (left_styles, right_styles) = side_styles(&lines, &theme);

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

                // —— 同步滚动：读上一帧状态，取出本帧要强加的偏移。——
                let sync_id = ui.id().with("diff-scroll-sync");
                let mut sync: ScrollSync = ui.data_mut(|d| d.get_temp(sync_id)).unwrap_or_default();
                let nudge = sync.nudge;
                let jump = sync.jump;
                let prev = sync.prev;
                sync.nudge = [None, None];
                sync.jump = [None, None];

                // 两个面板的输出（编辑框响应 + 匹配 y + 实际滚动偏移），出闭包后统一处理。
                let mut l_view: Option<(widgets::DiffAreaOutput, f32)> = None;
                let mut r_view: Option<(widgets::DiffAreaOutput, f32)> = None;

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
                                let mut sa = egui::ScrollArea::vertical()
                                    .id_salt("diff-l-sc")
                                    .min_scrolled_height(pin_h)
                                    .max_height(pin_h)
                                    .auto_shrink([false, false]);
                                // 上一帧对侧被用户滚动 → 镜像同样的增量；搜索跳转 → 绝对定位。
                                if let Some(y) = jump[0] {
                                    sa = sa.vertical_scroll_offset(y);
                                } else if let Some(d) = nudge[0] {
                                    sa = sa.vertical_scroll_offset((prev[0] + d).max(0.0));
                                }
                                let out = sa.show(ui, |ui| {
                                    widgets::code_area_diff(
                                        ui,
                                        &theme,
                                        "diff-l",
                                        &mut self.left,
                                        rows,
                                        &left_styles,
                                        min_inner_h,
                                        l_paint.as_ref(),
                                    )
                                });
                                l_view = Some((out.inner, out.state.offset.y));
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
                                let mut sa = egui::ScrollArea::vertical()
                                    .id_salt("diff-r-sc")
                                    .min_scrolled_height(pin_h)
                                    .max_height(pin_h)
                                    .auto_shrink([false, false]);
                                if let Some(y) = jump[1] {
                                    sa = sa.vertical_scroll_offset(y);
                                } else if let Some(d) = nudge[1] {
                                    sa = sa.vertical_scroll_offset((prev[1] + d).max(0.0));
                                }
                                let out = sa.show(ui, |ui| {
                                    widgets::code_area_diff(
                                        ui,
                                        &theme,
                                        "diff-r",
                                        &mut self.right,
                                        rows,
                                        &right_styles,
                                        min_inner_h,
                                        r_paint.as_ref(),
                                    )
                                });
                                r_view = Some((out.inner, out.state.offset.y));
                            },
                        );
                    });
                });

                // panel 的 body 闭包一定会执行，这里必然有值。
                let (l_area, l_scroll) = l_view.expect("左面板已渲染");
                let (r_area, r_scroll) = r_view.expect("右面板已渲染");

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

                // —— 同步滚动判定：镜像决策抽成纯函数（见 mirror_scroll），此处只接线。
                let forced = [
                    nudge[0].is_some() || jump[0].is_some(),
                    nudge[1].is_some() || jump[1].is_some(),
                ];
                sync.nudge = mirror_scroll(prev, [l_scroll, r_scroll], forced);
                sync.prev = [l_scroll, r_scroll];

                // —— 搜索跳转：导航帧从面板拿到当前匹配 y，下一帧把两侧一起滚到
                // 视口约 1/3 处（偏移镜像，天然带动另一侧）。——
                if self.search_nav {
                    let y = if cur_left.is_some() {
                        l_area.current_match_y
                    } else {
                        r_area.current_match_y
                    };
                    if let Some(y) = y {
                        let off = (y - pin_h / 3.0).max(0.0);
                        sync.jump = [Some(off), Some(off)];
                    }
                    self.search_nav = false;
                }
                // 有待强加的偏移就再画一帧，让它立即生效。
                if sync
                    .nudge
                    .iter()
                    .chain(sync.jump.iter())
                    .any(Option::is_some)
                {
                    ui.ctx().request_repaint();
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

    /// 镜像的是**增量**：右侧被夹在 145 时用户把左侧从 2000 滚到 2100，
    /// 给右侧的必须是 +100（它自己去饱和），而不是绝对位置。
    #[test]
    fn mirror_passes_deltas_not_absolute_offsets() {
        let out = mirror_scroll([2000.0, 145.0], [2100.0, 145.0], [false, false]);
        assert_eq!(out, [None, Some(100.0)]);
    }

    /// 回归锁定：右侧在夹点附近小幅回滚 −10，左侧（在 2000）收到的必须是 −10 ——
    /// 老实现镜像绝对偏移，会把左侧从 2000 **瞬移**到 135，用户看到的就是
    /// 「左边滚过的位置全丢了 / 滚动没有效果」。
    #[test]
    fn short_side_scroll_never_teleports_the_long_side() {
        let out = mirror_scroll([2000.0, 145.0], [2000.0, 135.0], [false, false]);
        assert_eq!(out, [Some(-10.0), None]);
        if let [Some(d), _] = out {
            assert!(
                d.abs() < 50.0,
                "给长侧的量级应是用户手滚的增量，出现大跳变（{d}）说明退回了绝对镜像"
            );
        }
    }

    /// 被程序强加（镜像/跳转）的一侧，其偏移变化不算用户滚动 —— 否则镜像回来的
    /// 偏移被当成新滚动，左右互相追着抖、每帧强制重绘（软渲染机器直接卡死）。
    #[test]
    fn forced_side_changes_do_not_echo_back() {
        // 右侧本帧是被镜像推过去的（forced），它的变化不得再镜像回左侧
        let out = mirror_scroll([100.0, 0.0], [100.0, 100.0], [false, true]);
        assert_eq!(out, [None, None], "回声没有被切断，会形成互相追赶的抖动环");
        // 静止帧什么都不产生
        assert_eq!(
            mirror_scroll([50.0, 50.0], [50.0, 50.0], [false, false]),
            [None, None]
        );
    }
}
