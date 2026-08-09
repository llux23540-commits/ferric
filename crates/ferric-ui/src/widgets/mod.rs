//! 复用 UI 组件与样式辅助（对齐 Ferric 设计原型）。

use crate::icons;
use crate::theme::Theme;
use egui::{
    vec2, Align, Align2, Button, Color32, CornerRadius, DragValue, FontId, Frame, Layout, Margin,
    Response, RichText, Sense, Stroke, TextEdit, Ui,
};

pub mod code_editor;

/// JSON 语法高亮：把文本按 token 着色生成 `LayoutJob`（供 TextEdit 的 layouter 使用，
/// 边编辑边高亮）。键=主色、字符串=绿、数字=琥珀、true/false=红、null/标点=弱色。
///
/// `line_height` 为 `Some` 时按给定行高排版（编辑区的行距设置）；`None` 用字体自带行高。
pub(crate) fn json_highlight(
    text: &str,
    font_id: &FontId,
    theme: &Theme,
    line_height: Option<f32>,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let num_col = if theme.dark {
        Color32::from_rgb(0xe0, 0xb0, 0x62)
    } else {
        Color32::from_rgb(0xb0, 0x6f, 0x00)
    };
    let mut job = LayoutJob::default();
    let mk = |c: Color32| TextFormat {
        font_id: font_id.clone(),
        color: c,
        line_height,
        ..Default::default()
    };
    let b = text.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut run = 0usize; // 待输出的「标点/空白」默认段起点
    let is_ws = |c: u8| c == b' ' || c == b'\t' || c == b'\n' || c == b'\r';
    while i < n {
        let c = b[i];
        if c == b'"' {
            if run < i {
                job.append(&text[run..i], 0.0, mk(theme.muted));
            }
            let start = i;
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let end = i.min(n);
            let mut k = end;
            while k < n && is_ws(b[k]) {
                k += 1;
            }
            let is_key = k < n && b[k] == b':';
            let col = if is_key {
                theme.accent_strong
            } else {
                theme.ok
            };
            job.append(&text[start..end], 0.0, mk(col));
            run = end;
        } else if c == b'-' || c.is_ascii_digit() {
            if run < i {
                job.append(&text[run..i], 0.0, mk(theme.muted));
            }
            let start = i;
            i += 1;
            while i < n {
                let d = b[i];
                if d.is_ascii_digit()
                    || d == b'.'
                    || d == b'e'
                    || d == b'E'
                    || d == b'+'
                    || d == b'-'
                {
                    i += 1;
                } else {
                    break;
                }
            }
            job.append(&text[start..i], 0.0, mk(num_col));
            run = i;
        } else if c == b't' || c == b'f' || c == b'l' || c == b'n' {
            let rest = &text[i..];
            let (lit, col) = if rest.starts_with("true") || rest.starts_with("false") {
                (if c == b't' { 4 } else { 5 }, theme.danger)
            } else if rest.starts_with("null") {
                (4, theme.muted)
            } else {
                (0, theme.muted)
            };
            if lit > 0 {
                if run < i {
                    job.append(&text[run..i], 0.0, mk(theme.muted));
                }
                job.append(&text[i..i + lit], 0.0, mk(col));
                i += lit;
                run = i;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    if run < n {
        job.append(&text[run..], 0.0, mk(theme.muted));
    }
    job
}

// ---- 文本标签 ----

/// 区块小标题（fg_soft）。
pub fn field_label(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(theme.fg_soft));
}

// ---- 按钮 ----

/// 主色实心按钮（可带图标）。
pub fn primary_button(ui: &mut Ui, theme: &Theme, text: &str) -> Response {
    btn(ui, theme, None, text, Variant::Primary)
}

/// 主色按钮 + 图标。
pub fn primary_icon(ui: &mut Ui, theme: &Theme, icon: char, text: &str) -> Response {
    btn(ui, theme, Some(icon), text, Variant::Primary)
}

/// 描边默认按钮。
pub fn ghost_button(ui: &mut Ui, theme: &Theme, text: &str) -> Response {
    btn(ui, theme, None, text, Variant::Default)
}

/// 弱化（subtle）按钮：无边透明，muted 文字，hover 才有底。
pub fn subtle_button(ui: &mut Ui, theme: &Theme, icon: Option<char>, text: &str) -> Response {
    btn(ui, theme, icon, text, Variant::Subtle)
}

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Primary,
    Default,
    Subtle,
}

fn btn(ui: &mut Ui, theme: &Theme, icon: Option<char>, text: &str, v: Variant) -> Response {
    let (fill, stroke, fg) = match v {
        Variant::Primary => (theme.accent, Stroke::NONE, Color32::WHITE),
        Variant::Default => (theme.code_bg, Stroke::NONE, theme.fg_soft),
        Variant::Subtle => (Color32::TRANSPARENT, Stroke::NONE, theme.muted),
    };
    let desired = vec2(0.0, 38.0);
    let padding = if matches!(v, Variant::Subtle) {
        12.0
    } else {
        16.0
    };

    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(padding, 8.0);
        let label: egui::WidgetText = match icon {
            Some(ch) => {
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &ch.to_string(),
                    0.0,
                    egui::TextFormat {
                        font_id: egui::FontId::new(16.0, icons::family()),
                        color: fg,
                        valign: Align::Center,
                        ..Default::default()
                    },
                );
                job.append(
                    &format!("  {text}"),
                    0.0,
                    egui::TextFormat {
                        font_id: egui::FontId::proportional(13.5),
                        color: fg,
                        valign: Align::Center,
                        ..Default::default()
                    },
                );
                job.into()
            }
            None => RichText::new(text).size(13.5).color(fg).into(),
        };
        ui.add(
            Button::new(label)
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(10))
                .min_size(desired),
        )
    })
    .inner
}

/// 图标按钮（正方形，muted → hover 变 fg + border 底）。
pub fn icon_btn(ui: &mut Ui, theme: &Theme, ch: char, size: f32) -> Response {
    // 方块跟着字号走，别写死 38：侧栏底部那排按图标 18px 算出来是 30，
    // 比原来的 38 少占一圈，底部整体轻了一档；而 24px 图标处（对比工具的关闭）
    // 算出来仍是 40，与原尺寸基本一致，不会改动那边的观感。
    let side = (size * 1.65).max(26.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), theme.border);
    }
    let color = if resp.hovered() {
        theme.fg
    } else {
        theme.muted
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        ch,
        egui::FontId::new(size, icons::family()),
        color,
    );
    resp
}

// ---- 分段控件（segmented pill）----

/// 分段控件：返回被点击的新选项索引（未变则 None）。
pub fn seg(ui: &mut Ui, theme: &Theme, opts: &[&str], selected: usize) -> Option<usize> {
    let mut clicked = None;
    Frame::NONE
        .fill(theme.code_bg)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                for (i, o) in opts.iter().enumerate() {
                    let on = i == selected;
                    let fill = if on {
                        theme.accent_soft
                    } else {
                        Color32::TRANSPARENT
                    };
                    let col = if on { theme.accent_strong } else { theme.muted };
                    ui.spacing_mut().button_padding = vec2(14.0, 6.0);
                    let r = ui.add(
                        Button::new(RichText::new(*o).size(13.0).color(col))
                            .fill(fill)
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(7))
                            .min_size(vec2(0.0, 30.0)),
                    );
                    if r.clicked() {
                        clicked = Some(i);
                    }
                }
            });
        });
    clicked
}

/// 复选“药丸”：带勾选框的开关。返回是否切换。
pub fn pill_toggle(ui: &mut Ui, theme: &Theme, on: bool, label: &str) -> bool {
    let col = if on { theme.accent_strong } else { theme.muted };
    let mut job = egui::text::LayoutJob::default();
    let box_ch = if on { icons::CHECK } else { ' ' };
    job.append(
        &box_ch.to_string(),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::new(13.0, icons::family()),
            color: if on { theme.accent } else { theme.faint },
            valign: Align::Center,
            ..Default::default()
        },
    );
    job.append(
        &format!(" {label}"),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(12.5),
            color: col,
            valign: Align::Center,
            ..Default::default()
        },
    );
    let fill = if on { theme.accent_soft } else { theme.code_bg };
    ui.add(
        Button::new(job)
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 32.0)),
    )
    .clicked()
}

// ---- 数字步进器 ----

/// `− value +` 步进器。返回是否变化。
pub fn num_field(
    ui: &mut Ui,
    theme: &Theme,
    value: &mut i64,
    min: i64,
    max: i64,
    step: i64,
) -> bool {
    let mut changed = false;
    Frame::NONE
        .fill(theme.code_bg)
        .corner_radius(CornerRadius::same(11))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                if icon_flat(ui, theme, icons::MINUS, 44.0).clicked() {
                    *value = (*value - step).clamp(min, max);
                    changed = true;
                }
                ui.allocate_ui_with_layout(
                    vec2(72.0, 44.0),
                    Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        let mut v = *value;
                        let r = ui.add(
                            DragValue::new(&mut v)
                                .range(min..=max)
                                .update_while_editing(false),
                        );
                        if r.changed() {
                            *value = v.clamp(min, max);
                            changed = true;
                        }
                    },
                );
                if icon_flat(ui, theme, icons::PLUS, 44.0).clicked() {
                    *value = (*value + step).clamp(min, max);
                    changed = true;
                }
            });
        });
    changed
}

/// 无边扁平图标按钮（步进器内部用）。
fn icon_flat(ui: &mut Ui, theme: &Theme, ch: char, w: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, theme.border);
    }
    let color = if resp.hovered() {
        theme.fg
    } else {
        theme.muted
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        ch,
        egui::FontId::new(17.0, icons::family()),
        color,
    );
    resp
}

// ---- 代码区 / 输入 ----

/// 等宽多行编辑框：包在主题化容器里（code_bg 底 + 极浅细边 + 圆角），
/// 内部 TextEdit 透明，避免"白色空白"。`editable=false` 时内容仅供选中复制。
pub fn code_area(
    ui: &mut Ui,
    id: &str,
    text: &mut String,
    editable: bool,
    rows: usize,
) -> Response {
    let fill = ui.visuals().extreme_bg_color;
    let border = ui.visuals().window_stroke; // border_2，很浅
    let accent = ui.visuals().hyperlink_color; // = accent
    let out = Frame::NONE
        .fill(fill)
        .stroke(border)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 12)) // 舒适内边距，文字不贴边
        .show(ui, |ui| {
            if editable {
                ui.add(
                    TextEdit::multiline(text)
                        .id_salt(id)
                        .desired_width(f32::INFINITY)
                        .desired_rows(rows)
                        .code_editor()
                        .frame(egui::Frame::NONE),
                )
            } else {
                // 只读：以不可变缓冲呈现——仍可选中/复制，键入不会改动内容。
                let mut ro = text.as_str();
                ui.add(
                    TextEdit::multiline(&mut ro)
                        .id_salt(id)
                        .desired_width(f32::INFINITY)
                        .desired_rows(rows)
                        .code_editor()
                        .frame(egui::Frame::NONE),
                )
            }
        });
    // 首次聚焦时不要全选默认文本：把光标折叠到文本末尾。
    if out.inner.gained_focus() {
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), out.inner.id) {
            let end = egui::text::CCursor::new(text.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), out.inner.id);
        }
    }
    // 聚焦时显示主色环
    if out.inner.has_focus() {
        ui.painter().rect_stroke(
            out.response.rect,
            CornerRadius::same(10),
            Stroke::new(1.5_f32, accent),
            egui::StrokeKind::Inside,
        );
    }
    out.inner
}

/// code_bg 卡片面板：标题头（左标题 + 右侧自定义内容）+ 任意主体内容，
/// SQL 格式化 / JSON→YAML / 对比等页面的统一版式。
pub fn panel(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    trailing: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui),
) {
    Frame::NONE
        .fill(theme.code_bg)
        .corner_radius(CornerRadius::same(12))
        .show(ui, |ui| {
            Frame::NONE
                .inner_margin(Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(title)
                                .size(11.0)
                                .family(egui::FontFamily::Monospace)
                                .color(theme.faint),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), trailing);
                    });
                });
            Frame::NONE.inner_margin(Margin::same(4)).show(ui, body);
        });
}

/// 差异行样式：`segs` 拼起来应等于该行文本；不一致（如输入后的一帧延迟）时该行退回无样式。
#[derive(Clone)]
pub struct DiffLineStyle {
    /// 整行底色（TRANSPARENT = 无）。
    pub bg: Color32,
    /// emph 片段的字符级标记背景。
    pub mark: Color32,
    pub segs: Vec<ferric_core::diff::Seg>,
}

impl DiffLineStyle {
    fn matches(&self, line: &str) -> bool {
        let mut rest = line;
        for s in &self.segs {
            match rest.strip_prefix(s.text.as_str()) {
                Some(r) => rest = r,
                None => return false,
            }
        }
        rest.is_empty()
    }
}

/// 搜索高亮参数：`matches` 为本面板全文中各匹配的 byte 区间（升序、互不重叠），
/// `current` 为「当前匹配」在 `matches` 中的下标（当前匹配不在本面板时为 `None`）。
pub struct SearchPaint<'a> {
    pub matches: &'a [(usize, usize)],
    pub current: Option<usize>,
}

/// [`code_area_diff`] 的返回：编辑框响应 + 当前搜索匹配的纵向位置。
pub struct DiffAreaOutput {
    pub response: Response,
    /// 当前搜索匹配所在视觉行相对滚动内容顶部的 y；
    /// 调用方据此给外层 ScrollArea 设置跳转偏移（无当前匹配时为 `None`）。
    pub current_match_y: Option<f32>,
}

/// 等宽多行编辑框（外观同 [`code_area`]），按 `line_styles` 就地渲染 diff 高亮：
/// 改动行整行底色横向铺满（折行的续行同底色），emph 片段画字符级标记。
/// `min_inner_h` 为内容区最小高度：行数除不尽的余数由白框撑满补齐，
/// 保证框体高度精确等于外部预算、左右两栏底边严格对齐。
/// `search` 提供搜索匹配区间时，在 diff 样式之上叠加匹配底色。
#[allow(clippy::too_many_arguments)] // 单一调用方（diff 视图），拆参数结构体只添噪音
pub fn code_area_diff(
    ui: &mut Ui,
    theme: &Theme,
    id: &str,
    text: &mut String,
    rows: usize,
    line_styles: &[DiffLineStyle],
    min_inner_h: f32,
    search: Option<&SearchPaint<'_>>,
) -> DiffAreaOutput {
    let fill = ui.visuals().extreme_bg_color;
    let border = ui.visuals().window_stroke; // border_2，很浅
    let accent = ui.visuals().hyperlink_color; // = accent
    let fg = theme.fg;
    // 搜索匹配底色：普通匹配用主色浅底，当前匹配用半透明主色加深（更醒目），
    // 与代码编辑器的搜索配色观感保持一致。
    let hit_bg = theme.accent_soft;
    let cur_bg = theme.accent.gamma_multiply(0.55);
    let out = Frame::NONE
        .fill(fill)
        .stroke(border)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_min_height(min_inner_h);
            // 行底色占位：先占一个绘制槽，TextEdit 画完后回填，保证底色在文字下方。
            let bg_idx = ui.painter().add(egui::Shape::Noop);

            let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let plain = egui::TextFormat {
                    font_id: font_id.clone(),
                    color: fg,
                    ..Default::default()
                };
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = wrap_width;
                let lines: Vec<&str> = buf.as_str().split('\n').collect();
                let mut line_start = 0usize; // 行首在全文中的 byte 偏移
                for (i, line) in lines.iter().enumerate() {
                    let line_end = line_start + line.len();
                    // 落在本行内的搜索区间（换算成行内偏移；跨行匹配按行裁剪）。
                    let mut hits: Vec<(usize, usize, bool)> = Vec::new();
                    if let Some(sp) = search {
                        for (mi, &(s, e)) in sp.matches.iter().enumerate() {
                            if e <= line_start {
                                continue;
                            }
                            if s >= line_end {
                                break;
                            }
                            hits.push((
                                s.max(line_start) - line_start,
                                e.min(line_end) - line_start,
                                sp.current == Some(mi),
                            ));
                        }
                        // 匹配区间在输入后的一帧内可能落后于文本（同 DiffLineStyle 的
                        // 一帧延迟问题）：丢掉不再落在 char 边界上的区间，避免切片 panic。
                        hits.retain(|&(a, b, _)| {
                            line.is_char_boundary(a) && line.is_char_boundary(b)
                        });
                    }
                    let style = line_styles.get(i).filter(|s| s.matches(line));
                    if hits.is_empty() {
                        // 无搜索命中：按 diff 段直接排版（原路径）。
                        match style {
                            Some(style) => {
                                for seg in &style.segs {
                                    let mut fmt = plain.clone();
                                    if seg.emph {
                                        fmt.background = style.mark;
                                    }
                                    job.append(&seg.text, 0.0, fmt);
                                }
                            }
                            None => job.append(line, 0.0, plain.clone()),
                        }
                    } else {
                        // 命中行：diff 段与搜索区间可能交叉，把两组边界合并后逐子段排版；
                        // 底色优先级：当前匹配 > 普通匹配 > diff 字符标记。
                        let base: Vec<(usize, usize, Color32)> = match style {
                            Some(style) => {
                                let mut off = 0usize;
                                style
                                    .segs
                                    .iter()
                                    .map(|seg| {
                                        let s = off;
                                        off += seg.text.len();
                                        let c = if seg.emph {
                                            style.mark
                                        } else {
                                            Color32::TRANSPARENT
                                        };
                                        (s, off, c)
                                    })
                                    .collect()
                            }
                            None => vec![(0, line.len(), Color32::TRANSPARENT)],
                        };
                        let mut cuts: Vec<usize> =
                            Vec::with_capacity((base.len() + hits.len()) * 2);
                        for &(a, b, _) in &base {
                            cuts.push(a);
                            cuts.push(b);
                        }
                        for &(a, b, _) in &hits {
                            cuts.push(a);
                            cuts.push(b);
                        }
                        cuts.sort_unstable();
                        cuts.dedup();
                        // 切分点含全部边界：每个 [a,b) 子段必然整体处于同一 diff 段、同一搜索状态。
                        for w in cuts.windows(2) {
                            let (a, b) = (w[0], w[1]);
                            let seg_bg = base
                                .iter()
                                .find(|&&(s, e, _)| s <= a && b <= e)
                                .map_or(Color32::TRANSPARENT, |&(_, _, c)| c);
                            let hit = hits.iter().find(|&&(s, e, _)| s <= a && a < e);
                            let mut fmt = plain.clone();
                            fmt.background = match hit {
                                Some(&(_, _, true)) => cur_bg,
                                Some(_) => hit_bg,
                                None => seg_bg,
                            };
                            job.append(&line[a..b], 0.0, fmt);
                        }
                    }
                    if i + 1 < lines.len() {
                        job.append("\n", 0.0, plain.clone());
                    }
                    line_start = line_end + 1;
                }
                ui.fonts_mut(|f| f.layout_job(job))
            };

            let edit = TextEdit::multiline(text)
                .id_salt(id)
                .desired_width(f32::INFINITY)
                .desired_rows(rows)
                .code_editor()
                .frame(egui::Frame::NONE)
                .layouter(&mut layouter)
                .show(ui);

            // 回填整行底色：按 galley 视觉行映射逻辑行（同 code_area_seamless 的行号逻辑）。
            let inner = ui.max_rect();
            let cur_lines: Vec<&str> = text.split('\n').collect();
            let mut shapes = Vec::new();
            let mut logical = 0usize;
            for grow in edit.galley.rows.iter() {
                let styled = line_styles
                    .get(logical)
                    .filter(|s| s.bg != Color32::TRANSPARENT)
                    .filter(|s| {
                        cur_lines
                            .get(logical)
                            .copied()
                            .is_some_and(|l| s.matches(l))
                    });
                if let Some(style) = styled {
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(inner.left(), edit.galley_pos.y + grow.rect().min.y),
                        egui::pos2(inner.right(), edit.galley_pos.y + grow.rect().max.y),
                    );
                    shapes.push(egui::Shape::rect_filled(
                        rect.expand2(vec2(6.0, 0.0)),
                        CornerRadius::same(3),
                        style.bg,
                    ));
                }
                if grow.ends_with_newline {
                    logical += 1;
                }
            }
            ui.painter().set(bg_idx, egui::Shape::Vec(shapes));

            // 当前搜索匹配的 y：galley 内坐标换算为「相对 Frame 外沿（= 滚动内容顶部）」。
            // 匹配区间可能比文本旧一帧，用 get 兜底避免切在 char 中间。
            let current_match_y = search
                .and_then(|sp| sp.current.and_then(|ci| sp.matches.get(ci)))
                .and_then(|&(s, _)| text.get(..s))
                .map(|prefix| {
                    let ccursor = egui::text::CCursor::new(prefix.chars().count());
                    let row_y = edit.galley.pos_from_cursor(ccursor).min.y;
                    // inner.top() 是内容顶，减去 inner_margin 顶得 Frame 外沿。
                    edit.galley_pos.y + row_y - (inner.top() - 12.0)
                });

            (edit.response.response, current_match_y)
        });
    let (response, current_match_y) = out.inner;
    // 键盘（Tab）聚焦时不要全选默认文本：把光标折叠到文本末尾。
    //
    // ⚠️ 只对**非点击**的聚焦做：点击聚焦时 TextEdit 已把光标放在点击处，
    // 再折到文末会触发「滚动到光标」——视图瞬移到文本末尾，左右同步又把另一栏
    // 也带走，用户看到的就是「一点击位置就飞了，选择根本没法用」。
    if response.gained_focus() && !response.is_pointer_button_down_on() {
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), response.id) {
            let end = egui::text::CCursor::new(text.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), response.id);
        }
    }
    // 聚焦时显示主色环
    if response.has_focus() {
        ui.painter().rect_stroke(
            out.response.rect,
            CornerRadius::same(10),
            Stroke::new(1.5_f32, accent),
            egui::StrokeKind::Inside,
        );
    }
    DiffAreaOutput {
        response,
        current_match_y,
    }
}

/// 代码盒子：field 底 + 右上角复制按钮覆盖，展示只读文本。返回复制点击。
pub fn code_box(ui: &mut Ui, theme: &Theme, id: &str, text: &str, min_rows: usize) -> bool {
    let mut copied = false;
    Frame::NONE
        .fill(theme.code_bg)
        .stroke(Stroke::new(1.0_f32, theme.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin {
            left: 16,
            right: 44,
            top: 14,
            bottom: 12,
        })
        .show(ui, |ui| {
            let mut owned = text.to_owned();
            code_area(ui, id, &mut owned, false, min_rows);
            // 右上角复制
            let r = ui.max_rect();
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(r.right() - 30.0, r.top() - 2.0),
                vec2(34.0, 34.0),
            );
            if ui
                .put(
                    btn_rect,
                    Button::new(icons::text(icons::COPY, 16.0, theme.muted))
                        .fill(theme.field)
                        .stroke(Stroke::new(1.0_f32, theme.border))
                        .corner_radius(CornerRadius::same(8)),
                )
                .clicked()
            {
                copied = true;
            }
        });
    copied
}

// ---- 状态行 ----

/// 圆角卡片：柔和阴影 + 最浅发丝线（不再是明显方框）。
pub fn card<R>(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    // 阴影取全局 Visuals（theme.apply 统一定义）：软件光栅化环境会把它清零，
    // 硬编码在这里的话就成了漏网的「灰雾」。
    let shadow = ui.visuals().window_shadow;
    Frame::NONE
        .fill(theme.bg)
        .stroke(Stroke::new(1.0_f32, theme.border))
        .corner_radius(CornerRadius::same(14))
        .shadow(shadow)
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// 工具条图标按钮（32×32，可选 active/primary，带 tooltip）。
pub fn tb_icon_btn(
    ui: &mut Ui,
    theme: &Theme,
    ch: char,
    active: bool,
    primary: bool,
    tip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let hovered = resp.hovered();
    let fill = if active {
        theme.accent_soft
    } else if hovered {
        theme.border
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    }
    let color = if active {
        theme.accent_strong
    } else if primary {
        theme.accent
    } else if hovered {
        theme.fg
    } else {
        theme.muted
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        ch,
        FontId::new(17.0, icons::family()),
        color,
    );
    resp.on_hover_text(tip)
}

/// 工具条文字按钮：与 [`tb_icon_btn`] 同款样式（32×32、透明底、选中高亮），
/// 但渲染一小段等宽文字（用于「2 / 4」这类缩进标签，取代药丸段控）。
pub fn tb_text_btn(ui: &mut Ui, theme: &Theme, label: &str, active: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let hovered = resp.hovered();
    let fill = if active {
        theme.accent_soft
    } else if hovered {
        theme.border
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    }
    let color = if active {
        theme.accent_strong
    } else if hovered {
        theme.fg
    } else {
        theme.muted
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(13.0, egui::FontFamily::Monospace),
        color,
    );
    resp.on_hover_text(tip)
}

/// 工具条竖分隔。
pub fn tb_sep(ui: &mut Ui, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(11.0, 32.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        (rect.center().y - 9.0)..=(rect.center().y + 9.0),
        Stroke::new(1.0_f32, theme.border_2),
    );
}

/// 状态行文字（成功用主色 ●，错误用危险色 ▲）。
pub fn status_line(ui: &mut Ui, theme: &Theme, ok: bool, text: &str) {
    let (glyph, color) = if ok {
        (icons::CIRCLE_CHECK, theme.ok)
    } else {
        (icons::CIRCLE_ALERT, theme.danger)
    };
    ui.horizontal(|ui| {
        ui.label(icons::text(glyph, 13.0, color));
        ui.label(RichText::new(text).size(11.5).color(color));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 点击对比面板聚焦时，光标必须落在**点击处附近**，绝不能被折到文末 ——
    /// 折到文末会触发「滚动到光标」，视图瞬移到底部，左右同步再把另一栏也带走，
    /// 用户看到的就是「一点击位置就飞了 / 选择没法用」（Windows 实测回归）。
    /// 折叠到文末只允许发生在键盘（Tab）聚焦，用于避开首次全选。
    #[test]
    fn clicking_diff_pane_keeps_cursor_at_click_not_end() {
        let ctx = egui::Context::default();
        crate::theme::Theme::light().apply(&ctx);
        let mut text: String = (0..200).map(|i| format!("line {i} content\n")).collect();
        let total = text.chars().count();
        let styles: Vec<DiffLineStyle> = Vec::new();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 400.0));
        let at = egui::pos2(120.0, 40.0); // 面板顶部第二三行附近
        let mut edit_id = None;

        let frames: Vec<Vec<egui::Event>> = vec![
            vec![],
            vec![egui::Event::PointerMoved(at)],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            vec![],
        ];
        for events in frames {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let out = code_area_diff(
                        ui,
                        &crate::theme::Theme::light(),
                        "click-probe",
                        &mut text,
                        20,
                        &styles,
                        380.0,
                        None,
                    );
                    edit_id = Some(out.response.id);
                },
            );
        }
        let state = egui::text_edit::TextEditState::load(&ctx, edit_id.unwrap())
            .expect("点击后应有编辑状态");
        let cur = state
            .cursor
            .char_range()
            .expect("点击后应有光标")
            .primary
            .index;
        assert!(
            cur.0 < total / 2,
            "点击面板顶部后光标跑到了后半段（{cur:?} / 共 {total}）—— \
             多半又被折叠到文末，视图会瞬移"
        );
    }
}
