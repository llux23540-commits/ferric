//! 时间戳转换视图（3 卡片：当前 / 时间戳→时间 / 时间→时间戳）。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::widgets;
use egui::{ComboBox, RichText, TextEdit, Ui};
use ferric_core::timestamp::{self, Precision};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 时区搜索框的高度（内边距撑出来的，见下面 `TextEdit::margin`）。
const TZ_SEARCH_H: f32 = 36.0;
/// 时区列表最多显示多高。一行约 22px，360px ≈ 16 行 —— 筛完的结果要能一眼看全，
/// 不能只露出一两条让人再去滚。
const TZ_LIST_H: f32 = 360.0;

/// 下拉里一行时区怎么显示：配了中文的带上中文名（`Asia/Shanghai · 上海`），
/// 没配的就只有英文标识 —— 让人一眼认出自己要的那个，而不是在 590 行英文里数。
fn tz_label(name: &str) -> String {
    match timestamp::zh_name(name) {
        Some(zh) => format!("{name} · {zh}"),
        None => name.to_string(),
    }
}

#[derive(Serialize, Deserialize)]
struct TimestampDraft {
    tz: String,
    ts_input: String,
    date_input: String,
}

pub struct TimestampTool {
    tz: chrono_tz::Tz,
    tz_filter: String,
    /// 下拉刚展开、待把焦点送进搜索框。
    tz_focus_pending: bool,
    ts_input: String,
    ts_output: String,
    date_input: String,
    date_output: String,
    /// 当前时间戳是否实时刷新；暂停时显示 `paused_ms` 的定格值。
    running: bool,
    paused_ms: i64,
}

impl Default for TimestampTool {
    fn default() -> Self {
        Self {
            tz: chrono_tz::Asia::Shanghai,
            tz_filter: String::new(),
            tz_focus_pending: false,
            ts_input: String::new(),
            ts_output: String::new(),
            date_input: "2025-07-08 12:03:05".to_owned(),
            date_output: String::new(),
            running: true,
            paused_ms: 0,
        }
    }
}

/// 只读字段样式的展示框 + 复制按钮。
fn readonly_field(ui: &mut Ui, theme: &crate::theme::Theme, value: &str, placeholder: &str) {
    egui::Frame::NONE
        .fill(theme.code_bg)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (txt, col) = if value.is_empty() {
                (placeholder, theme.faint)
            } else {
                (value, theme.fg_soft)
            };
            ui.label(RichText::new(txt).monospace().size(13.5).color(col));
        });
}

impl Tool for TimestampTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "timestamp",
            name: "时间戳",
            group: "转换",
            desc: "Unix 时间戳与日期互转，自动识别秒 / 毫秒，附本地 / UTC / 常用时区。",
            icon: crate::icons::CLOCK,
            keywords: &["timestamp", "unix", "时间戳", "时间", "date", "时区"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        // 实时刷新：对齐到下一个 100ms 边界再重绘（+5ms 余量确保跨过边界）。
        // 相比固定间隔，采样点不会在秒内漂移，长时间挂着也不会出现跳秒 / 卡顿。
        // 暂停时显示定格值，且不再请求重绘（零开销）。
        let now_ms = if self.running {
            let v = timestamp::now(Precision::Millis);
            let wait = 100 - (v % 100) + 5;
            ui.ctx()
                .request_repaint_after(Duration::from_millis(wait as u64));
            v
        } else {
            self.paused_ms
        };

        // ---- 卡1：当前 Unix 时间戳（秒级 / 毫秒级同时显示） ----
        widgets::card(ui, &theme, |ui| {
            ui.horizontal(|ui| {
                widgets::field_label(ui, &theme, "当前 Unix 时间戳");
                ui.add_space(10.0);
                if widgets::pill_toggle(ui, &theme, self.running, "实时刷新") {
                    self.running = !self.running;
                    if !self.running {
                        self.paused_ms = now_ms; // 定格当前值
                    }
                }
                if !self.running {
                    ui.add_space(6.0);
                    ui.label(RichText::new("已暂停").size(12.0).color(theme.muted));
                }
            });
            ui.add_space(6.0);
            let rows: [(&str, i64); 2] =
                [("秒级 · 10 位", now_ms / 1000), ("毫秒级 · 13 位", now_ms)];
            for (label, value) in rows {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [96.0, 20.0],
                        egui::Label::new(RichText::new(label).size(12.0).color(theme.muted)),
                    );
                    ui.add_space(8.0);
                    ui.add_sized(
                        [150.0, 26.0],
                        egui::Label::new(
                            RichText::new(value.to_string())
                                .monospace()
                                .size(20.0)
                                .color(theme.fg),
                        ),
                    );
                    ui.add_space(10.0);
                    if widgets::subtle_button(ui, &theme, Some(crate::icons::COPY), "复制")
                        .clicked()
                    {
                        shared.copy(ui.ctx(), value.to_string());
                    }
                });
            }
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                widgets::field_label(ui, &theme, "目标时区");
                ui.add_space(4.0);
                let combo = ComboBox::from_id_salt("tz-combo")
                    .selected_text(tz_label(self.tz.name()))
                    .width(260.0)
                    // ComboBox 默认是 `CloseOnClick`：点弹层里的**任何**东西都会把它关掉，
                    // 包括这个搜索框 —— 于是搜索框永远拿不到焦点，一个字也打不进去。
                    // 改成只在点到弹层外面时才关；选中条目由下面手动关闭。
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    // egui 会把弹层整体套进一个 `max_height = spacing.combo_height`（默认 200）
                    // 的 ScrollArea —— 搜索框也在里面，于是会跟着列表一起滚走，
                    // 而且 200px 一共装不下几行。把外层上限抬到「搜索框 + 列表」之上，
                    // 外层就永远滚不起来：搜索框钉在顶上，只有下面的列表滚。
                    .height(TZ_SEARCH_H + TZ_LIST_H + 16.0)
                    .show_ui(ui, |ui| {
                        // 搜索框放大：默认单行 TextEdit 只有 ~20px 高，挤在下拉顶端很难点中。
                        // 靠内边距把它撑到 ~36px，字号也提一档。
                        let search = ui.add(
                            TextEdit::singleline(&mut self.tz_filter)
                                .desired_width(f32::INFINITY)
                                .margin(egui::Margin::symmetric(9, 9))
                                .font(egui::FontId::proportional(14.0))
                                .hint_text("搜索时区：上海 / 北京 / Shanghai / 亚洲"),
                        );
                        // 刚展开时把光标直接放进搜索框：展开下拉本就是为了找时区，
                        // 还要再点一下才能打字是白费一步。
                        if self.tz_focus_pending {
                            search.request_focus();
                            self.tz_focus_pending = false;
                        }
                        ui.add_space(4.0);
                        let f = self.tz_filter.clone();
                        // 全量列出（约 590 个），超长部分靠下拉内滚动，不截断。
                        egui::ScrollArea::vertical()
                            .max_height(TZ_LIST_H)
                            .show(ui, |ui| {
                                // 列表高度**固定**，不随筛出多少条伸缩。
                                // 否则每敲一个字弹层就跳一下高度：本来瞄准的那一行会跑掉，
                                // 越筛越难点。宁可留一块空白，也要让位置是稳的。
                                ui.set_min_height(TZ_LIST_H);
                                let mut hits = 0usize;
                                for z in chrono_tz::TZ_VARIANTS
                                    .iter()
                                    .filter(|z| timestamp::tz_matches(z.name(), &f))
                                {
                                    hits += 1;
                                    let label = tz_label(z.name());
                                    if ui.selectable_value(&mut self.tz, *z, label).clicked() {
                                        // 改了关闭策略之后，选完得自己收起来
                                        egui::Popup::close_all(ui.ctx());
                                    }
                                }
                                if hits == 0 {
                                    // 一条都没匹配上时给句话，否则就是一片空白，
                                    // 看着像卡住了而不是「没找到」。
                                    ui.add_space(10.0);
                                    ui.label(
                                        RichText::new("没有匹配的时区，换个词试试")
                                            .size(12.5)
                                            .color(theme.muted),
                                    );
                                }
                            });
                    });
                if combo.response.clicked() {
                    // 每次重新展开都从空搜索开始，免得沿用上次的过滤条件让人以为列表少了
                    self.tz_filter.clear();
                    self.tz_focus_pending = true;
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("当前系统时区：{}", timestamp::system_offset()))
                    .size(12.0)
                    .color(theme.muted),
            );
        });
        ui.add_space(14.0);

        // ---- 卡2：时间戳 → 目标时间 ----
        widgets::card(ui, &theme, |ui| {
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "时间戳 → 目标时间");
                cols[0].add_space(6.0);
                cols[0].horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.ts_input)
                            .desired_width(180.0)
                            .hint_text("10 / 13 位，自动识别"),
                    );
                    if widgets::primary_button(ui, &theme, "转换").clicked() {
                        self.ts_output = match self.ts_input.trim().parse::<i64>() {
                            Ok(ts) => {
                                // 按位数自动识别精度：≥13 位按毫秒，否则按秒。
                                let precision =
                                    if self.ts_input.trim().trim_start_matches('-').len() >= 13 {
                                        Precision::Millis
                                    } else {
                                        Precision::Seconds
                                    };
                                timestamp::to_datetime(ts, precision, self.tz)
                                    .unwrap_or_else(|e| format!("错误：{e}"))
                            }
                            Err(_) => "错误：请输入整数时间戳".to_owned(),
                        };
                    }
                });
                cols[1].horizontal(|ui| {
                    widgets::field_label(ui, &theme, "转换后的时间");
                    if !self.ts_output.is_empty()
                        && !self.ts_output.starts_with("错误")
                        && widgets::subtle_button(ui, &theme, Some(crate::icons::COPY), "复制")
                            .clicked()
                    {
                        shared.copy(ui.ctx(), self.ts_output.clone());
                    }
                });
                cols[1].add_space(6.0);
                readonly_field(&mut cols[1], &theme, &self.ts_output, "转换后的时间");
            });
        });
        ui.add_space(14.0);

        // ---- 卡3：目标时间 → 时间戳 ----
        widgets::card(ui, &theme, |ui| {
            ui.columns(2, |cols| {
                widgets::field_label(&mut cols[0], &theme, "目标时间 → 时间戳（自动识别格式）");
                cols[0].add_space(6.0);
                cols[0].horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.date_input)
                            .desired_width(220.0)
                            .hint_text("2025-07-08 12:03:05 / 2025/7/8 / 20250708120305"),
                    );
                    if widgets::primary_button(ui, &theme, "转换").clicked() {
                        self.date_output = timestamp::parse_flexible(&self.date_input, self.tz)
                            .map(|ts| ts.to_string())
                            .unwrap_or_else(|e| format!("错误：{e}"));
                    }
                });
                cols[1].horizontal(|ui| {
                    widgets::field_label(ui, &theme, "转换后的时间戳");
                    if !self.date_output.is_empty()
                        && !self.date_output.starts_with("错误")
                        && widgets::subtle_button(ui, &theme, Some(crate::icons::COPY), "复制")
                            .clicked()
                    {
                        shared.copy(ui.ctx(), self.date_output.clone());
                    }
                });
                cols[1].add_space(6.0);
                readonly_field(&mut cols[1], &theme, &self.date_output, "转换后的时间戳");
            });
        });
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&TimestampDraft {
            tz: self.tz.name().to_owned(),
            ts_input: self.ts_input.clone(),
            date_input: self.date_input.clone(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<TimestampDraft>(data) {
            if let Ok(tz) = d.tz.parse::<chrono_tz::Tz>() {
                self.tz = tz;
            }
            self.ts_input = d.ts_input;
            self.date_input = d.date_input;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{vec2, Event, Pos2, Rect};

    fn press(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn click_at(pos: Pos2) -> Vec<Vec<Event>> {
        vec![
            vec![Event::PointerMoved(pos)],
            vec![press(pos, true)],
            vec![press(pos, false)],
        ]
    }

    /// 在渲染输出里找一段文字，返回它的位置（用来定位控件，避免硬编码坐标）。
    fn find_text(tool: &mut TimestampTool, needle: &str) -> Option<Pos2> {
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let theme = crate::theme::Theme::light();
        theme.apply(&ctx);
        let mut shared = Shared::new(theme);
        let mut found = None;
        for i in 0..3 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
                    time: Some(i as f64 * 0.05),
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| tool.ui(ui, &mut shared));
                },
            );
            for cs in &out.shapes {
                if let egui::Shape::Text(t) = &cs.shape {
                    if t.galley.text().contains(needle) {
                        found = Some(t.galley.rect.translate(t.pos.to_vec2()).center());
                    }
                }
            }
        }
        found
    }

    /// 驱动一次完整交互：点开时区下拉 → 点搜索框 → 键入 `typing`。
    /// 返回（工具最终状态, 最后一帧的渲染输出, 搜索框提示文字的位置）。
    fn open_combo_and_type(typing: &str) -> (TimestampTool, egui::FullOutput, Option<Pos2>) {
        open_combo_and_type_on(typing, 700.0)
    }

    /// 同上，但可以指定窗口高度（矮窗口下弹层放不下的场景）。
    fn open_combo_and_type_on(
        typing: &str,
        screen_h: f32,
    ) -> (TimestampTool, egui::FullOutput, Option<Pos2>) {
        let mut tool = TimestampTool::default();
        let combo = find_text(&mut tool, "Asia/Shanghai").expect("没找到时区下拉按钮");

        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let theme = crate::theme::Theme::light();
        theme.apply(&ctx);
        let mut shared = Shared::new(theme);
        let mut frames: Vec<Vec<Event>> = Vec::new();
        frames.extend(click_at(combo));
        frames.push(vec![]);
        let mut search_pos: Option<Pos2> = None;
        let mut last = None;
        for i in 0..24usize {
            let events = frames.get(i).cloned().unwrap_or_default();
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, screen_h))),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| tool.ui(ui, &mut shared));
                },
            );
            // 下拉展开后会出现提示文字「搜索时区…」，据此拿到搜索框位置
            if search_pos.is_none() {
                for cs in &out.shapes {
                    if let egui::Shape::Text(t) = &cs.shape {
                        if t.galley.text().contains("搜索时区") {
                            let p = t.galley.rect.translate(t.pos.to_vec2()).center();
                            search_pos = Some(p);
                            // 点它，再键入
                            frames.resize(i + 1, Vec::new());
                            frames.extend(click_at(p));
                            frames.push(vec![]);
                            frames.push(vec![Event::Text(typing.to_owned())]);
                            frames.push(vec![]);
                        }
                    }
                }
            }
            last = Some(out);
        }
        (tool, last.expect("一帧都没跑"), search_pos)
    }

    /// 在下拉已展开的状态下，把鼠标放到列表上滚几下。
    /// 返回（滚动前的最后一帧, 滚动后的最后一帧, 搜索框位置）。
    fn open_combo_and_scroll_at(dy: f32, n: usize) -> (egui::FullOutput, egui::FullOutput, Pos2) {
        let mut tool = TimestampTool::default();
        let combo = find_text(&mut tool, "Asia/Shanghai").expect("没找到时区下拉按钮");

        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let theme = crate::theme::Theme::light();
        theme.apply(&ctx);
        let mut shared = Shared::new(theme);
        let mut frames: Vec<Vec<Event>> = Vec::new();
        frames.extend(click_at(combo));
        frames.push(vec![]);
        let mut search_pos: Option<Pos2> = None;
        let mut before = None;
        let mut scroll_from = usize::MAX;
        let mut last = None;
        for i in 0..(30 + n) {
            let events = frames.get(i).cloned().unwrap_or_default();
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| tool.ui(ui, &mut shared));
                },
            );
            if search_pos.is_none() {
                for cs in &out.shapes {
                    if let egui::Shape::Text(t) = &cs.shape {
                        if t.galley.text().contains("搜索时区") {
                            let p = t.galley.rect.translate(t.pos.to_vec2()).center();
                            search_pos = Some(p);
                            // 把指针挪到搜索框下方的列表上，再滚几帧
                            let on_list = p + vec2(0.0, dy);
                            frames.resize(i + 1, Vec::new());
                            frames.push(vec![Event::PointerMoved(on_list)]);
                            scroll_from = i + 2;
                            for _ in 0..n {
                                frames.push(vec![Event::MouseWheel {
                                    unit: egui::MouseWheelUnit::Point,
                                    delta: vec2(0.0, -120.0),
                                    modifiers: Default::default(),
                                    phase: egui::TouchPhase::Move,
                                }]);
                            }
                            frames.push(vec![]);
                        }
                    }
                }
            }
            if i + 1 == scroll_from {
                before = Some(out);
                continue;
            }
            last = Some(out);
        }
        (
            before.expect("下拉没展开"),
            last.expect("一帧都没跑"),
            search_pos.unwrap(),
        )
    }

    /// 收集一帧里画出的全部文字。
    fn texts(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    /// 这段文字是不是下拉里的一行时区（`Asia/Shanghai` 或 `Asia/Shanghai · 上海`）。
    ///
    /// 必须拿真实时区名核对，不能只看含不含 `/` —— 界面上「10 / 13 位，自动识别」
    /// 这类文案也带斜杠，混进来会把「能看到几条」数成一个好看但假的数字。
    fn is_zone_row(s: &str) -> bool {
        let name = s.split(" · ").next().unwrap_or(s);
        chrono_tz::TZ_VARIANTS.iter().any(|z| z.name() == name)
    }

    /// 一帧里**真正可见**的时区行（文字矩形完全落在自己的裁剪框内），按纵坐标排序。
    ///
    /// - 被裁掉的行照样在 shapes 里，用户却看不到 —— 数「能看到几条」必须按裁剪框算；
    /// - `below_y` 用来排掉下拉按钮自己的选中文字（它也是个时区名，永远画在弹层上方，
    ///   混进来会让「首行」永远不变）。
    fn visible_zone_rows(out: &egui::FullOutput, below_y: f32) -> Vec<(f32, String)> {
        let mut rows: Vec<(f32, String)> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => {
                    let r = t.galley.rect.translate(t.pos.to_vec2());
                    let s = t.galley.text();
                    (is_zone_row(s) && r.top() > below_y && cs.clip_rect.contains_rect(r))
                        .then(|| (r.top(), s.to_owned()))
                }
                _ => None,
            })
            .collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        rows
    }

    /// 列表区域的裁剪框 —— 就是那个内层滚动区的视口。
    /// 锚在列表里的任意一段文字上（时区行，或一条都没匹配上时的那句提示），
    /// 这样 0 结果的情形也量得到。
    fn list_viewport(out: &egui::FullOutput, below_y: f32) -> Option<Rect> {
        out.shapes.iter().find_map(|cs| match &cs.shape {
            egui::Shape::Text(t) => {
                let s = t.galley.text();
                let r = t.galley.rect.translate(t.pos.to_vec2());
                let inside = is_zone_row(s) || s.starts_with("没有匹配");
                (inside && r.top() > below_y).then_some(cs.clip_rect)
            }
            _ => None,
        })
    }

    /// 弹层外框 —— 面积最大的那个矩形（排掉滚动内容那个上万像素高的假矩形）。
    fn popup_frame(out: &egui::FullOutput) -> Option<Rect> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.rect.height() <= 800.0 => Some(r.rect),
                _ => None,
            })
            .max_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
    }

    /// 列表区域的可视高度。
    fn list_viewport_h(out: &egui::FullOutput, below_y: f32) -> Option<f32> {
        list_viewport(out, below_y).map(|r| r.height())
    }

    /// 搜索框里那段文字画在哪个纵坐标上。
    /// 没输入时是提示文字，输入之后提示会消失，得改认输入的内容本身。
    fn search_y(out: &egui::FullOutput, filter: &str) -> Option<f32> {
        out.shapes.iter().find_map(|cs| match &cs.shape {
            egui::Shape::Text(t) => {
                let s = t.galley.text();
                let hit = if filter.is_empty() {
                    s.contains("搜索时区")
                } else {
                    s == filter
                };
                hit.then(|| t.galley.rect.translate(t.pos.to_vec2()).top())
            }
            _ => None,
        })
    }

    /// 提示文字「搜索时区…」当前画在哪个纵坐标上。
    fn hint_y(out: &egui::FullOutput) -> Option<f32> {
        search_y(out, "")
    }

    /// 用户报的问题：**时区搜索框打不了字**。
    ///
    /// 搜索框在下拉弹层里：点开下拉 → 点搜索框 → 键入，`tz_filter` 应当收到字符。
    /// 控件位置从渲染输出里找（认「Asia/Shanghai」与提示文字），不写死坐标。
    #[test]
    fn timezone_search_accepts_typing() {
        let (tool, _, search_pos) = open_combo_and_type("sha");
        assert!(search_pos.is_some(), "下拉没展开，找不到搜索框");
        assert_eq!(
            tool.tz_filter, "sha",
            "时区搜索框收不到键入（实际：{:?}）",
            tool.tz_filter
        );
    }

    /// 用户要求：搜索要能用中文。键入「上海」，列表里应当**只剩** Asia/Shanghai。
    #[test]
    fn timezone_search_filters_by_chinese() {
        let (tool, out, _) = open_combo_and_type("上海");
        assert_eq!(tool.tz_filter, "上海");

        let zones: std::collections::BTreeSet<_> = texts(&out)
            .into_iter()
            .filter(|t| t.starts_with("Asia/") || t.starts_with("Europe/"))
            .collect();
        assert!(
            zones.contains("Asia/Shanghai · 上海"),
            "中文搜「上海」没列出 Asia/Shanghai，实际列表：{zones:?}"
        );
        assert!(
            !zones.iter().any(|z| z.starts_with("Asia/Tokyo")),
            "中文搜「上海」却把东京也列出来了，过滤没生效：{zones:?}"
        );
    }

    /// 用户要求：下拉一次至少要能看全 5 条结果。
    ///
    /// 数的是**可见**行（文字完全落在裁剪框内）—— 被裁掉的行照样在 shapes 里，
    /// 按 shapes 数会数出一个漂亮但假的数字。
    #[test]
    fn timezone_dropdown_shows_at_least_five_rows() {
        let (_, out, _) = open_combo_and_type("");
        let hint = hint_y(&out).expect("下拉没展开");
        let rows = visible_zone_rows(&out, hint);
        assert!(
            rows.len() >= 5,
            "下拉一次只看得到 {} 条时区，至少要 5 条：{:?}",
            rows.len(),
            rows.iter().map(|(_, s)| s).collect::<Vec<_>>()
        );
    }

    /// 用户要求：列表区域要够高，筛出来的结果要能一眼看全。
    ///
    /// 量的是时区行所在的裁剪框 —— 也就是内层滚动区真正拿到的视口高度。
    /// 这同时是「搜索框钉在顶部」的结构保证：egui 的 ComboBox 会把弹层整体
    /// 套进一个 `max_height = spacing.combo_height`（默认 200）的 ScrollArea，
    /// 搜索框也在里面。只要外层放得下「搜索框 + 整个列表」，外层就没有可滚的余量，
    /// 搜索框也就不可能被滚走。
    #[test]
    fn timezone_list_area_is_tall_enough() {
        let (_, out, _) = open_combo_and_type("");
        let hint = hint_y(&out).expect("下拉没展开");
        let h = list_viewport_h(&out, hint).expect("找不到列表区域");
        assert!(
            h >= TZ_LIST_H - 40.0,
            "列表可视高度只有 {h:.0}px（要 ≈{TZ_LIST_H:.0}px）——\
             外层弹层没放下整个列表，搜索框会被挤进可滚区域"
        );
    }

    /// 滚动列表时，搜索框必须原地不动、且始终完整可见。
    ///
    /// 说明：headless 下试过把指针放在搜索框上、列表上、以及一路滚到列表末尾，
    /// egui 的外层弹层滚动区都不响应滚轮，所以这条**抓不到**「外层可滚」的回归 ——
    /// 真正守住那点的是上面那条高度测试。这条只是把「列表滚了、搜索框没动」钉住。
    #[test]
    fn search_box_stays_put_while_list_scrolls() {
        let (before, after, _) = open_combo_and_scroll_at(90.0, 4);

        let y0 = hint_y(&before).expect("滚动前找不到搜索框");
        let y1 = hint_y(&after).expect("滚动后搜索框不见了");

        // 先确认列表**确实**滚动了，否则这条测试等于没测
        let top_before = visible_zone_rows(&before, y0)
            .first()
            .map(|(_, s)| s.clone());
        let top_after = visible_zone_rows(&after, y1)
            .first()
            .map(|(_, s)| s.clone());
        assert!(
            top_before.is_some() && top_before != top_after,
            "列表根本没滚动（首行始终是 {top_before:?}），这条测试没意义"
        );

        assert!(
            (y0 - y1).abs() < 1.0,
            "搜索框跟着列表滚走了：{y0:.1} → {y1:.1}"
        );
    }

    /// 用户要求：搜索框要更高一点。取包住提示文字的最小矩形（即输入框边框）来量。
    #[test]
    fn timezone_search_box_is_tall_enough() {
        let (_, out, search_pos) = open_combo_and_type("");
        let pos = search_pos.expect("下拉没展开");
        let box_h = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.rect.contains(pos) => Some(r.rect),
                _ => None,
            })
            .map(|r| (r.area(), r.height()))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, h)| h)
            .expect("没找到搜索框的边框矩形");
        // egui 默认单行 TextEdit 约 20px；这里靠内边距撑到 30 以上才好点。
        assert!(box_h >= 30.0, "时区搜索框太矮了：{box_h:.1}px（要求 ≥30）");
    }

    /// 窗口很矮时下拉仍要能用 —— 弹层比窗口还高的话，egui 会重新摆放它，
    /// 但结果必须依然是「至少看得到 5 条」，不能缩成一条缝。
    #[test]
    fn dropdown_stays_usable_in_a_short_window() {
        let (_, out, _) = open_combo_and_type_on("", 400.0);
        let hint = hint_y(&out).expect("矮窗口下搜索框不可见");
        let rows = visible_zone_rows(&out, hint);
        assert!(
            rows.len() >= 5,
            "窗口 400px 高时只看得到 {} 条时区",
            rows.len()
        );
    }

    /// 用户报的问题：**输入文字后显示就变了、高度变了**。
    ///
    /// 列表原本按内容伸缩：筛到只剩一条时视口从 340px 塌到 30px，整个弹层跳一下 ——
    /// 刚才瞄准的那一行就跑掉了。这条钉住「换任何筛选词，列表视口高度都一样」，
    /// 包括一条都没匹配上的情形。
    #[test]
    fn dropdown_height_is_stable_across_filters() {
        let cases = ["", "上海", "亚洲", "America", "zzzz"];
        let mut heights = Vec::new();
        for f in cases {
            let (_, out, _) = open_combo_and_type(f);
            let y = search_y(&out, f).unwrap_or_else(|| panic!("筛选「{f}」后找不到搜索框"));
            let h =
                list_viewport_h(&out, y).unwrap_or_else(|| panic!("筛选「{f}」后量不到列表区域"));
            let frame = popup_frame(&out).unwrap_or_else(|| panic!("筛选「{f}」后量不到弹层外框"));
            heights.push((f, h, (frame.width().round(), frame.height().round())));
        }
        let (_, base_h, base_frame) = heights[0];
        for (f, h, frame) in &heights {
            assert!(
                (h - base_h).abs() < 1.0,
                "筛选「{f}」后列表高度变成 {h:.0}px（没筛时是 {base_h:.0}px）—— 弹层会跳一下"
            );
            assert_eq!(
                *frame, base_frame,
                "筛选「{f}」后弹层外框变了（没筛时是 {base_frame:?}）—— 弹层会跳一下"
            );
        }
    }

    /// 一条都没匹配上时要说一句话，不能只留一片空白 —— 空白看着像卡住了。
    #[test]
    fn empty_result_shows_a_hint() {
        let (_, out, _) = open_combo_and_type("zzzz");
        assert!(
            texts(&out).iter().any(|t| t.starts_with("没有匹配")),
            "筛不到结果时没有任何提示"
        );
    }
}
