//! 复用 UI 组件与样式辅助（对齐 Ferric 设计原型）。

use crate::icons;
use crate::theme::Theme;
use egui::{
    vec2, Align, Align2, Button, Color32, DragValue, FontId, Frame, Layout, Margin, Response,
    RichText, Rounding, ScrollArea, Sense, Stroke, TextEdit, Ui,
};

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
                .rounding(Rounding::same(10.0))
                .min_size(desired),
        )
    })
    .inner
}

/// 图标按钮（正方形，muted → hover 变 fg + border 底）。
pub fn icon_btn(ui: &mut Ui, theme: &Theme, ch: char, size: f32) -> Response {
    let side = 38.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, Rounding::same(9.0), theme.border);
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
    Frame::none()
        .fill(theme.code_bg)
        .rounding(Rounding::same(9.0))
        .inner_margin(egui::Margin::same(3.0))
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
                            .rounding(Rounding::same(7.0))
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
            .rounding(Rounding::same(8.0))
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
    Frame::none()
        .fill(theme.code_bg)
        .rounding(Rounding::same(11.0))
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
        ui.painter().rect_filled(rect, Rounding::ZERO, theme.border);
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
    let out = Frame::none()
        .fill(fill)
        .stroke(border)
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(16.0, 12.0)) // 舒适内边距，文字不贴边
        .show(ui, |ui| {
            if editable {
                ui.add(
                    TextEdit::multiline(text)
                        .id_salt(id)
                        .desired_width(f32::INFINITY)
                        .desired_rows(rows)
                        .code_editor()
                        .frame(false),
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
                        .frame(false),
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
            Rounding::same(10.0),
            Stroke::new(1.5_f32, accent),
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
    Frame::none()
        .fill(theme.code_bg)
        .rounding(Rounding::same(12.0))
        .show(ui, |ui| {
            Frame::none()
                .inner_margin(Margin::symmetric(14.0, 8.0))
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
            Frame::none().inner_margin(Margin::same(4.0)).show(ui, body);
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

/// 等宽多行编辑框（外观同 [`code_area`]），按 `line_styles` 就地渲染 diff 高亮：
/// 改动行整行底色横向铺满（折行的续行同底色），emph 片段画字符级标记。
pub fn code_area_diff(
    ui: &mut Ui,
    theme: &Theme,
    id: &str,
    text: &mut String,
    rows: usize,
    line_styles: &[DiffLineStyle],
) -> Response {
    let fill = ui.visuals().extreme_bg_color;
    let border = ui.visuals().window_stroke; // border_2，很浅
    let accent = ui.visuals().hyperlink_color; // = accent
    let fg = theme.fg;
    let out = Frame::none()
        .fill(fill)
        .stroke(border)
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(16.0, 12.0))
        .show(ui, |ui| {
            // 行底色占位：先占一个绘制槽，TextEdit 画完后回填，保证底色在文字下方。
            let bg_idx = ui.painter().add(egui::Shape::Noop);

            let mut layouter = |ui: &Ui, buf: &str, wrap_width: f32| {
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let plain = egui::TextFormat {
                    font_id: font_id.clone(),
                    color: fg,
                    ..Default::default()
                };
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = wrap_width;
                let lines: Vec<&str> = buf.split('\n').collect();
                for (i, line) in lines.iter().enumerate() {
                    match line_styles.get(i).filter(|s| s.matches(line)) {
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
                    if i + 1 < lines.len() {
                        job.append("\n", 0.0, plain.clone());
                    }
                }
                ui.fonts(|f| f.layout_job(job))
            };

            let edit = TextEdit::multiline(text)
                .id_salt(id)
                .desired_width(f32::INFINITY)
                .desired_rows(rows)
                .code_editor()
                .frame(false)
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
                        egui::pos2(inner.left(), edit.galley_pos.y + grow.rect.min.y),
                        egui::pos2(inner.right(), edit.galley_pos.y + grow.rect.max.y),
                    );
                    shapes.push(egui::Shape::rect_filled(
                        rect.expand2(vec2(6.0, 0.0)),
                        Rounding::same(3.0),
                        style.bg,
                    ));
                }
                if grow.ends_with_newline {
                    logical += 1;
                }
            }
            ui.painter().set(bg_idx, egui::Shape::Vec(shapes));
            edit.response
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
            Rounding::same(10.0),
            Stroke::new(1.5_f32, accent),
        );
    }
    out.inner
}

/// 无边框贴底编辑区：透明背景直接融入主题背景（亮/暗自动跟随），
/// 文字用主题前景色（清晰可读），不画卡片框与聚焦环，铺满给定高度，超长时内部滚动。
/// 左侧带行号栏：按 galley 实际布局逐「逻辑行」编号，折行的续行不编号。
pub fn code_area_seamless(
    ui: &mut Ui,
    theme: &Theme,
    id: &str,
    text: &mut String,
    height: f32,
) -> Response {
    let inner_h = height.max(60.0);
    let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let rows = (inner_h / row_h).floor().max(3.0) as usize;

    // 行号栏宽度：位数 × 字宽 + 右侧间距，随内容行数自适应。
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let num_color = ui.visuals().weak_text_color();
    let line_count = text.split('\n').count().max(1);
    let digits = line_count.to_string().len().max(2);
    let char_w = ui.fonts(|f| f.glyph_width(&font_id, '0'));
    let gutter_w = char_w * digits as f32 + 12.0;

    let mut out = ScrollArea::vertical()
        .id_salt(format!("{id}-sc"))
        .max_height(inner_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(gutter_w);
                let edit = TextEdit::multiline(text)
                    .id_salt(id)
                    .desired_width(f32::INFINITY)
                    .desired_rows(rows)
                    .code_editor()
                    .text_color(theme.fg)
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .show(ui);
                // 按 galley 实际布局逐「逻辑行」绘制行号：折行的续行不编号，始终对齐。
                let painter = ui.painter();
                let nx = edit.galley_pos.x - 6.0; // 数字右缘，贴着文本左侧
                let mut logical = 1usize;
                let mut start_line = true;
                for grow in edit.galley.rows.iter() {
                    if start_line {
                        let y = edit.galley_pos.y + grow.rect.min.y;
                        painter.text(
                            egui::pos2(nx, y),
                            Align2::RIGHT_TOP,
                            logical.to_string(),
                            font_id.clone(),
                            num_color,
                        );
                        logical += 1;
                    }
                    start_line = grow.ends_with_newline;
                }
                edit.response
            })
            .inner
        });
    let resp = out.inner;

    // 拖选超出可视区上/下边缘时自动滚动（egui 原生不支持）：
    // 超出距离越大滚得越快（约每秒 10 倍超出距离），选区随内容滚动持续扩展。
    // 注意不能用 resp.dragged()：指针一离开编辑区它就变 false（egui 行为），
    // 改用「编辑器有焦点 + 左键按住」判定拖选中。
    let selecting = ui.input(|i| i.pointer.primary_down()) && (resp.dragged() || resp.has_focus());
    if selecting {
        if let Some(pos) = ui.ctx().pointer_latest_pos() {
            let view = out.inner_rect;
            let overshoot = if pos.y < view.top() {
                pos.y - view.top()
            } else if pos.y > view.bottom() {
                pos.y - view.bottom()
            } else {
                0.0
            };
            if overshoot != 0.0 {
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                // 速度随超出距离增大，保底 60px/s
                let speed = overshoot.signum() * (overshoot.abs() * 10.0).max(60.0);
                let max_off = (out.content_size.y - view.height()).max(0.0);
                let new_y = (out.state.offset.y + speed * dt).clamp(0.0, max_off);
                if (new_y - out.state.offset.y).abs() > f32::EPSILON {
                    out.state.offset.y = new_y;
                    out.state.store(ui.ctx(), out.id);
                }
                ui.ctx().request_repaint(); // 指针不动也要持续滚
            }
        }
    }
    // 首次聚焦时避免“全选默认文本”，但要折叠到**当前光标处（点击落点）**而非文本末尾，
    // 否则第一次点击会从末尾选到点击处。
    if resp.gained_focus() {
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), resp.id) {
            if let Some(range) = state.cursor.char_range() {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(range.primary)));
                state.store(ui.ctx(), resp.id);
            }
        }
    }
    resp
}

/// 代码盒子：field 底 + 右上角复制按钮覆盖，展示只读文本。返回复制点击。
pub fn code_box(ui: &mut Ui, theme: &Theme, id: &str, text: &str, min_rows: usize) -> bool {
    let mut copied = false;
    Frame::none()
        .fill(theme.code_bg)
        .stroke(Stroke::new(1.0_f32, theme.border))
        .rounding(Rounding::same(12.0))
        .inner_margin(egui::Margin {
            left: 16.0,
            right: 44.0,
            top: 14.0,
            bottom: 12.0,
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
                        .rounding(Rounding::same(8.0)),
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
    let shadow = egui::epaint::Shadow {
        offset: vec2(0.0, 3.0),
        blur: 16.0,
        spread: 0.0,
        color: Color32::from_black_alpha(if theme.dark { 55 } else { 16 }),
    };
    Frame::none()
        .fill(theme.bg)
        .stroke(Stroke::new(1.0_f32, theme.border))
        .rounding(Rounding::same(14.0))
        .shadow(shadow)
        .inner_margin(Margin::same(18.0))
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
        ui.painter().rect_filled(rect, Rounding::same(7.0), fill);
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
        ui.painter().rect_filled(rect, Rounding::same(7.0), fill);
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
