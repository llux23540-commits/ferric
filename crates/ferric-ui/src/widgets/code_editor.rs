//! 自研代码编辑器：可自由编辑 + 语法高亮 + **代码折叠**（单视图共存）。
//!
//! 设计：`text: String` 为唯一真相。每帧由源文本构建一份「可见文本」（被折叠的
//! 括号区间替换成占位 `⋯`）以及「可见↔源」字符段映射。galley / 光标 / 选区都作用在
//! **可见文本** 上（于是 egui 自带的光标运算天然跳过被折叠的行）；编辑时再把可见位置
//! 换算回源位置落到源 `String`。折叠区间由 JSON 花括号 / 方括号配对算出。
//!
//! 复用 egui 0.35 公开工具：`TextBuffer`（改写源串）、`CCursorRange::on_key_press`
//! （光标移动）、`TextCursorState::pointer_interaction`（鼠标选择）、
//! `paint_text_selection`/`paint_cursor_end`（选区/光标绘制）、`Galley::cursor_from_pos`。

use crate::theme::Theme;
use egui::text::{CCursor, CharIndex};
use egui::text_selection::text_cursor_state::{cursor_rect, TextCursorState};
use egui::text_selection::visuals::{paint_cursor_end, paint_text_selection};
use egui::text_selection::CCursorRange;
use egui::{
    pos2, vec2, Align2, Color32, Event, EventFilter, Key, Pos2, Rect, Response, Sense, Shape,
    TextBuffer, Ui, Vec2,
};
use std::collections::HashSet;

/// 编辑器持久状态（光标 + 折叠集 + 待映射源光标）。
#[derive(Clone, Default)]
struct EditorState {
    cursor: TextCursorState,
    /// 已折叠区间：以「开括号 `{`/`[` 的源字符下标」为键（编辑时随文本平移）。
    folded: HashSet<usize>,
    /// 结构性编辑后待重映射的源光标（下一帧重建后换算回可见坐标）。
    pending: Option<usize>,
    /// 输入法预编辑串当前占用的源字符区间（组字中；提交/取消后清空）。
    ime: Option<(usize, usize)>,
    /// 焦点意图闩锁：一旦在编辑器内交互就置真，直到在编辑器外点击才清除。
    /// 用于在低帧率软渲染 / 输入法激活导致的瞬时失焦后，下一帧稳健地重夺焦点。
    want_focus: bool,
    /// 调试：最近事件日志（临时）。
    dbg: Vec<String>,
}

/// 调试开关：在编辑器右上角画出焦点/事件，用于诊断输入法问题（默认关闭）。
const DEBUG_HUD: bool = false;

/// 一段可折叠区间（字符下标，均为源文本 char 下标）。
#[derive(Clone, Copy)]
struct Region {
    br: usize,    // 开括号 '{' / '[' 自身下标
    open: usize,  // 开括号之后（隐藏内容起点）
    close: usize, // 匹配的闭括号 '}' / ']' 下标（隐藏内容终点，闭括号本身可见）
}

/// 可见文本的一段：真实源文本段，或折叠占位段。
#[derive(Clone, Copy)]
enum SegKind {
    Real(usize), // 对应源字符起点
    Fold { br: usize, open: usize, close: usize },
}
#[derive(Clone, Copy)]
struct Seg {
    vis_start: usize,
    len: usize,
    kind: SegKind,
}

const PLACEHOLDER: &str = " ⋯ ";

/// 渲染一个可编辑、语法高亮、可折叠的 JSON 代码编辑器，铺满 `height`，返回交互 `Response`。
pub fn code_editor(
    ui: &mut Ui,
    theme: &Theme,
    id_source: &str,
    text: &mut String,
    height: f32,
) -> Response {
    let id = ui.make_persistent_id(id_source);
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let num_color = ui.visuals().weak_text_color();
    let arrow_color = theme.muted;
    let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font_id, '0'));
    let inner_h = height.max(60.0);

    let mut state: EditorState = ui.data_mut(|d| d.get_temp::<EditorState>(id)).unwrap_or_default();

    let out = egui::ScrollArea::vertical()
        .id_salt((id, "sc"))
        .max_height(inner_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ---- 1. 扫描源文本：折叠区间 + 换行位置 ----
            let chars: Vec<char> = text.chars().collect();
            let n = chars.len();
            let regions = scan_regions(&chars);
            let region_brs: HashSet<usize> = regions.iter().map(|r| r.br).collect();
            state.folded.retain(|br| region_brs.contains(br)); // 剪除失效锚点
            let src_nl = newline_positions(&chars);

            // ---- 2. 构建可见文本 + 段映射 ----
            let (vis, segs) = build_visible(&chars, n, &regions, &state.folded);
            let vis_chars_len = vis.chars().count();
            let vis_nl = newline_positions_str(&vis);

            // ---- 3. 高亮 galley（不换行）----
            let mut job = crate::widgets::json_highlight(&vis, &font_id, theme);
            job.wrap.max_width = f32::INFINITY;
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));

            // 行号栏宽度（含折叠箭头区）。
            let last_line = src_nl.len() + 1;
            let digits = last_line.to_string().len().max(2);
            let gutter_w = char_w * digits as f32 + 26.0;

            let content = Vec2::new(
                gutter_w + galley.size().x + 12.0,
                galley.size().y.max(inner_h),
            );
            let (rect, _) = ui.allocate_exact_size(content, Sense::hover());
            // 用本编辑器自己的 id 交互，`id` 才会被注册为「可聚焦」。
            let resp = ui.interact(rect, id, Sense::click_and_drag());
            let text_origin = rect.min + vec2(gutter_w, 0.0);

            // ---- 4. 待映射源光标（上一帧结构编辑后）→ 可见坐标 ----
            if let Some(src) = state.pending.take() {
                let v = map_src(&segs, src, vis_chars_len);
                state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(v))));
            }

            // ---- 5. 折叠箭头命中框 ----
            let mut arrows: Vec<(Rect, usize, bool)> = Vec::new(); // (hitbox, br, folded?)
            for r in &regions {
                if is_hidden(r.br, &segs) {
                    continue; // 该 header 被上层折叠盖住，不显示箭头
                }
                let v = map_src(&segs, r.br, vis_chars_len);
                let row = vis_nl.partition_point(|&p| p < v);
                if row >= galley.rows.len() {
                    continue;
                }
                let y = text_origin.y + galley.rows[row].pos.y;
                let hit = Rect::from_min_size(pos2(rect.left() + 2.0, y), vec2(16.0, row_h));
                arrows.push((hit, r.br, state.folded.contains(&r.br)));
            }

            // ---- 6. 交互：先处理折叠箭头点击，其次文本 ----
            let mut gutter_click = false;
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    for (hit, br, _) in &arrows {
                        if hit.contains(p) {
                            // 折叠前把当前光标换算到源，折叠后再映回，避免光标跳飞。
                            if let Some(rg) = state.cursor.range(&galley) {
                                let (s, _) = map_vis(&segs, rg.primary.index.0, n);
                                state.pending = Some(s);
                            }
                            if state.folded.contains(br) {
                                state.folded.remove(br);
                            } else {
                                state.folded.insert(*br);
                            }
                            gutter_click = true;
                            ui.ctx().request_repaint();
                            break;
                        }
                    }
                }
            }

            // 焦点闩锁：在编辑器内按下/点击/拖动 → 置意图真；在编辑器外按下 → 清除。
            // 然后每帧「只要有意图且当前没焦点」就重夺——这样即便低帧率或输入法激活造成瞬时
            // 失焦，下一帧也能立刻恢复（合成点击/软渲染下只认 clicked() 会漏掉）。
            let pressed_on_me =
                resp.clicked() || resp.dragged() || resp.is_pointer_button_down_on();
            let pressed_elsewhere =
                ui.input(|i| i.pointer.any_pressed()) && !resp.contains_pointer();
            if pressed_on_me && !gutter_click {
                state.want_focus = true;
            }
            if pressed_elsewhere {
                state.want_focus = false;
            }
            if state.want_focus && !resp.has_focus() {
                resp.request_focus();
            }
            let has_focus = resp.has_focus();

            // ---- 调试：无论是否聚焦都记录到达的输入事件 ----
            if DEBUG_HUD {
                let evs = ui.input(|i| i.events.clone());
                for e in &evs {
                    let d = match e {
                        Event::Text(t) => format!("Text {t:?}"),
                        Event::Ime(im) => format!("Ime {im:?}"),
                        Event::Key { key, pressed, .. } => format!("Key {key:?} p={pressed}"),
                        Event::Paste(_) => "Paste".to_owned(),
                        _ => continue,
                    };
                    state.dbg.push(d);
                }
                while state.dbg.len() > 9 {
                    state.dbg.remove(0);
                }
            }

            if has_focus {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        id,
                        EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        },
                    );
                });
            }

            // 鼠标：定位光标 / 选择（仅在正文区，避免行号栏拖拽误选）。
            if !gutter_click {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    if ptr.x >= text_origin.x - 2.0 {
                        let cursor_at = galley.cursor_from_pos(ptr - text_origin);
                        state
                            .cursor
                            .pointer_interaction(ui, &resp, cursor_at, &galley, resp.dragged());
                    }
                }
            }

            // ---- 7. 键盘 / 文本事件（仅聚焦时消费）----
            if has_focus && !gutter_click {
                let os = ui.ctx().os();
                let events = ui.input(|i| i.events.clone());
                let mut vrange = state
                    .cursor
                    .range(&galley)
                    .unwrap_or_else(|| CCursorRange::one(galley.end()));
                'ev: for ev in &events {
                    match ev {
                        Event::Text(t) if !t.is_empty() => {
                            if edit_replace(text, &segs, n, &mut state, &vrange, t) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Paste(t) if !t.is_empty() => {
                            if edit_replace(text, &segs, n, &mut state, &vrange, t) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Key {
                            key: Key::Enter,
                            pressed: true,
                            ..
                        } => {
                            if edit_replace(text, &segs, n, &mut state, &vrange, "\n") {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Key {
                            key: Key::Backspace,
                            pressed: true,
                            ..
                        } => {
                            if edit_backspace(text, &segs, n, &mut state, &vrange) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Key {
                            key: Key::Delete,
                            pressed: true,
                            ..
                        } => {
                            if edit_delete(text, &segs, n, vis_chars_len, &mut state, &vrange) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Copy => {
                            if let Some(s) = selection_src(text, &segs, n, &vrange) {
                                if !s.is_empty() {
                                    ui.ctx().copy_text(s);
                                }
                            }
                        }
                        Event::Cut => {
                            if let Some(s) = selection_src(text, &segs, n, &vrange) {
                                if !s.is_empty() {
                                    ui.ctx().copy_text(s);
                                    if edit_replace(text, &segs, n, &mut state, &vrange, "") {
                                        ui.ctx().request_repaint();
                                        break 'ev;
                                    }
                                }
                            }
                        }
                        Event::Ime(ime) => {
                            // 不 break：`Preedit("")` 与 `Commit(text)` 常在同一帧连续到达，
                            // 必须都处理，否则提交的文字被丢弃（中文永远上不了屏）。
                            edit_ime(text, &segs, n, &mut state, &vrange, ime);
                            ui.ctx().request_repaint();
                        }
                        Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            // 方向 / Home/End / 按词 / 全选：作用在可见 galley 上。
                            vrange.on_key_press(os, &galley, modifiers, *key);
                        }
                        _ => {}
                    }
                }
                if state.pending.is_none() {
                    state.cursor.set_char_range(Some(vrange));
                }
            }

            // ---- 8. 绘制：选区 → 文本 → 行号/箭头 → 光标 ----
            let mut galley = galley;
            if has_focus {
                if let Some(r) = state.cursor.range(&galley) {
                    paint_text_selection(&mut galley, ui.visuals(), &r, None);
                }
            }
            let painter = ui.painter().clone();
            painter.galley(text_origin, galley.clone(), theme.fg);

            // 行号：逐可见行显示对应的**源**行号（折叠处号码跳变，如 VS Code）。
            for (i, prow) in galley.rows.iter().enumerate() {
                let y = text_origin.y + prow.pos.y;
                let vstart = if i == 0 { 0 } else { vis_nl[i - 1] + 1 };
                let (src_ci, _) = map_vis(&segs, vstart, n);
                let src_line = src_nl.partition_point(|&p| p < src_ci) + 1;
                painter.text(
                    pos2(text_origin.x - 8.0, y),
                    Align2::RIGHT_TOP,
                    src_line.to_string(),
                    font_id.clone(),
                    num_color,
                );
            }

            // 折叠箭头。
            for (hit, _, folded) in &arrows {
                draw_arrow(&painter, *hit, *folded, arrow_color);
            }

            // 调试 HUD。
            if DEBUG_HUD {
                let vp = ui.clip_rect();
                let mono = egui::FontId::monospace(12.0);
                let mut y = vp.top() + 6.0;
                let hdr = format!(
                    "focus={} ime_state={:?} os={:?}",
                    has_focus,
                    state.ime,
                    ui.ctx().os()
                );
                for line in std::iter::once(hdr).chain(state.dbg.iter().cloned()) {
                    painter.rect_filled(
                        egui::Rect::from_min_size(pos2(vp.right() - 430.0, y), vec2(426.0, 15.0)),
                        2.0,
                        Color32::from_black_alpha(190),
                    );
                    painter.text(
                        pos2(vp.right() - 426.0, y),
                        Align2::LEFT_TOP,
                        line,
                        mono.clone(),
                        Color32::from_rgb(0x8f, 0xff, 0x8f),
                    );
                    y += 16.0;
                }
            }

            // 光标 + 输入法候选框定位。
            if has_focus {
                if let Some(r) = state.cursor.range(&galley) {
                    let cr = cursor_rect(&galley, &r.primary, row_h).translate(text_origin.to_vec2());
                    paint_cursor_end(&painter, ui.visuals(), cr);
                    // 上报候选框位置：焦点期间恒定，OS 才会把输入法窗贴到光标处并开启 IME。
                    ui.ctx().output_mut(|o| {
                        o.ime = Some(egui::output::IMEOutput {
                            rect,
                            cursor_rect: cr,
                            should_interrupt_composition: false,
                        })
                    });
                }
            }

            resp
        });

    ui.data_mut(|d| d.insert_temp(id, state));
    out.inner
}

// ============================ 折叠 / 映射 辅助 ============================

/// 扫描源文本，找出所有跨行的括号配对（可折叠区间），忽略字符串内的括号。
fn scan_regions(chars: &[char]) -> Vec<Region> {
    let mut regions = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (开括号下标, 行号)
    let mut line = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            if c == '\n' {
                line += 1;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => stack.push((i, line)),
            '}' | ']' => {
                if let Some((bi, bline)) = stack.pop() {
                    if bline != line {
                        regions.push(Region {
                            br: bi,
                            open: bi + 1,
                            close: i,
                        });
                    }
                }
            }
            '\n' => line += 1,
            _ => {}
        }
    }
    regions
}

/// 由源文本 + 折叠集构建可见文本与段映射。
fn build_visible(
    chars: &[char],
    n: usize,
    regions: &[Region],
    folded: &HashSet<usize>,
) -> (String, Vec<Seg>) {
    // 取出已折叠区间，按 open 排序，剔除被外层折叠盖住的嵌套区间。
    let mut active: Vec<Region> = regions.iter().copied().filter(|r| folded.contains(&r.br)).collect();
    active.sort_by_key(|r| r.open);
    let mut top: Vec<Region> = Vec::new();
    let mut cover_end = 0usize;
    for r in active {
        if !top.is_empty() && r.open <= cover_end {
            continue; // 落在上一折叠区间内 → 已隐藏
        }
        top.push(r);
        cover_end = r.close;
    }

    let ph_len = PLACEHOLDER.chars().count();
    let mut vis = String::new();
    let mut segs: Vec<Seg> = Vec::new();
    let mut si = 0usize;
    let mut vlen = 0usize;
    for r in &top {
        if r.open > si {
            let len = r.open - si;
            segs.push(Seg {
                vis_start: vlen,
                len,
                kind: SegKind::Real(si),
            });
            vis.extend(&chars[si..r.open]);
            vlen += len;
        }
        segs.push(Seg {
            vis_start: vlen,
            len: ph_len,
            kind: SegKind::Fold {
                br: r.br,
                open: r.open,
                close: r.close,
            },
        });
        vis.push_str(PLACEHOLDER);
        vlen += ph_len;
        si = r.close;
    }
    if si < n {
        let len = n - si;
        segs.push(Seg {
            vis_start: vlen,
            len,
            kind: SegKind::Real(si),
        });
        vis.extend(&chars[si..n]);
    }
    (vis, segs)
}

/// 可见 char 下标 → (源 char 下标, 若落在占位内部则给出需展开的 br)。
fn map_vis(segs: &[Seg], vis_ci: usize, n: usize) -> (usize, Option<usize>) {
    let mut fold: Option<(usize, usize)> = None; // (open, br)
    for s in segs {
        if vis_ci >= s.vis_start && vis_ci <= s.vis_start + s.len {
            match s.kind {
                SegKind::Real(src) => return (src + (vis_ci - s.vis_start), None),
                SegKind::Fold { br, open, .. } => {
                    if fold.is_none() {
                        fold = Some((open, br));
                    }
                }
            }
        }
    }
    match fold {
        Some((open, br)) => (open, Some(br)),
        None => (n, None),
    }
}

/// 源 char 下标 → 可见 char 下标（落在隐藏区间则贴到占位起点）。
fn map_src(segs: &[Seg], src_ci: usize, vis_len: usize) -> usize {
    for s in segs {
        if let SegKind::Real(src) = s.kind {
            if src_ci >= src && src_ci <= src + s.len {
                return s.vis_start + (src_ci - src);
            }
        }
    }
    for s in segs {
        if let SegKind::Fold { open, close, .. } = s.kind {
            if src_ci >= open && src_ci <= close {
                return s.vis_start;
            }
        }
    }
    vis_len
}

/// 该源下标（开括号）是否被某个折叠段隐藏。
fn is_hidden(br: usize, segs: &[Seg]) -> bool {
    for s in segs {
        if let SegKind::Fold { open, close, .. } = s.kind {
            if br > open - 1 && br < close {
                return true;
            }
        }
    }
    false
}

fn newline_positions(chars: &[char]) -> Vec<usize> {
    chars
        .iter()
        .enumerate()
        .filter(|(_, &c)| c == '\n')
        .map(|(i, _)| i)
        .collect()
}
fn newline_positions_str(s: &str) -> Vec<usize> {
    s.char_indices()
        .scan(0usize, |ci, (_, c)| {
            let cur = *ci;
            *ci += 1;
            Some((cur, c))
        })
        .filter(|(_, c)| *c == '\n')
        .map(|(i, _)| i)
        .collect()
}

// ============================ 编辑（可见→源）============================

/// 平移折叠锚点：在源位置 `point` 处发生 `delta`（char 增量）后调整。
fn shift_folds(state: &mut EditorState, point: usize, delta: isize) {
    if delta == 0 {
        return;
    }
    state.folded = state
        .folded
        .iter()
        .map(|&a| {
            if a >= point {
                (a as isize + delta).max(0) as usize
            } else {
                a
            }
        })
        .collect();
}

/// 用 `ins` 替换当前选区（选区为空则纯插入）。返回 true 表示已改写源文本、需重建。
fn edit_replace(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
    ins: &str,
) -> bool {
    let (p, pu) = map_vis(segs, vrange.primary.index.0, n);
    let (s, su) = map_vis(segs, vrange.secondary.index.0, n);
    if pu.is_some() || su.is_some() {
        if let Some(br) = pu {
            state.folded.remove(&br);
        }
        if let Some(br) = su {
            state.folded.remove(&br);
        }
        return true; // 展开后下一帧重建，本次编辑略过
    }
    let (lo, hi) = (p.min(s), p.max(s));
    if hi > lo {
        text.delete_char_range(CharIndex(lo)..CharIndex(hi));
    }
    let added = text.insert_text(ins, CharIndex(lo));
    let delta = added as isize - (hi - lo) as isize;
    shift_folds(state, lo, delta);
    state.pending = Some(lo + added);
    true
}

fn edit_backspace(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
) -> bool {
    if !vrange.is_empty() {
        return edit_replace(text, segs, n, state, vrange, "");
    }
    let vi = vrange.primary.index.0;
    if vi == 0 {
        return false;
    }
    // 光标紧跟在占位之后 → 展开而非删除隐藏内容。
    if let (_, Some(br)) = map_vis(segs, vi - 1, n) {
        state.folded.remove(&br);
        return true;
    }
    let (p, _) = map_vis(segs, vi, n);
    if p == 0 {
        return false;
    }
    text.delete_char_range(CharIndex(p - 1)..CharIndex(p));
    shift_folds(state, p - 1, -1);
    state.pending = Some(p - 1);
    true
}

fn edit_delete(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    vis_len: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
) -> bool {
    if !vrange.is_empty() {
        return edit_replace(text, segs, n, state, vrange, "");
    }
    let vi = vrange.primary.index.0;
    // 光标紧贴占位之前 → 展开而非删除隐藏内容。
    if vi < vis_len {
        if let (_, Some(br)) = map_vis(segs, vi + 1, n) {
            state.folded.remove(&br);
            return true;
        }
    }
    let (p, pu) = map_vis(segs, vi, n);
    if pu.is_some() {
        if let Some(br) = pu {
            state.folded.remove(&br);
        }
        return true;
    }
    if p >= n {
        return false;
    }
    text.delete_char_range(CharIndex(p)..CharIndex(p + 1));
    shift_folds(state, p, -1);
    state.pending = Some(p);
    true
}

/// 处理输入法事件（组字 Preedit / 提交 Commit）。参照 egui `TextEdit`：预编辑串直接写入
/// 源文本并被下一帧渲染，每次新 Preedit 先删除上次预编辑区间再重插；Commit 落定为正式文本。
fn edit_ime(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
    ime: &egui::ImeEvent,
) -> bool {
    // 起点优先级：① 有活动预编辑 → 复用其起点并先删旧串；② 同一帧内上一个 IME 事件遗留
    // 的锚点（关键：`Preedit("")` + `Commit(text)` 常同帧到达，提交须接在清空之后的位置）；
    // ③ 全新组字 → 由当前光标映射到源。
    let start = if let Some((s, e)) = state.ime.take() {
        text.delete_char_range(CharIndex(s)..CharIndex(e));
        shift_folds(state, s, -((e - s) as isize));
        s
    } else if let Some(p) = state.pending {
        p
    } else {
        let (p, pu) = map_vis(segs, vrange.primary.index.0, n);
        if let Some(br) = pu {
            state.folded.remove(&br); // 光标落在占位内 → 先展开
            return true;
        }
        p
    };
    match ime {
        egui::ImeEvent::Preedit { text: t, .. } => {
            if t.is_empty() {
                state.ime = None;
                state.pending = Some(start);
            } else {
                let added = text.insert_text(t, CharIndex(start));
                shift_folds(state, start, added as isize);
                state.ime = Some((start, start + added));
                state.pending = Some(start + added);
            }
            true
        }
        egui::ImeEvent::Commit(t) => {
            let added = text.insert_text(t, CharIndex(start));
            shift_folds(state, start, added as isize);
            state.pending = Some(start + added);
            true
        }
        _ => {
            state.pending = Some(start);
            false
        }
    }
}

/// 取当前选区对应的**源文本**切片（用于复制/剪切）；跨占位则返回含隐藏内容的整段。
fn selection_src(text: &str, segs: &[Seg], n: usize, vrange: &CCursorRange) -> Option<String> {
    let (p, _) = map_vis(segs, vrange.primary.index.0, n);
    let (s, _) = map_vis(segs, vrange.secondary.index.0, n);
    let (lo, hi) = (p.min(s), p.max(s));
    if hi <= lo {
        return Some(String::new());
    }
    Some(text.chars().skip(lo).take(hi - lo).collect())
}

// ============================ 绘制 ============================

/// 在 `hit` 行号栏区绘制折叠箭头：折叠时 ▸（右），展开时 ▾（下）。
fn draw_arrow(painter: &egui::Painter, hit: Rect, folded: bool, color: Color32) {
    let c = hit.center();
    let pts: [Pos2; 3] = if folded {
        [
            pos2(c.x - 3.0, c.y - 4.0),
            pos2(c.x - 3.0, c.y + 4.0),
            pos2(c.x + 4.0, c.y),
        ]
    } else {
        [
            pos2(c.x - 4.0, c.y - 3.0),
            pos2(c.x + 4.0, c.y - 3.0),
            pos2(c.x, c.y + 4.0),
        ]
    };
    painter.add(Shape::convex_polygon(pts.to_vec(), color, egui::Stroke::NONE));
}
