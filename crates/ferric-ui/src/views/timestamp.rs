//! 时间戳转换视图（3 卡片：当前 / 时间戳→时间 / 时间→时间戳）。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::widgets;
use egui::{ComboBox, RichText, TextEdit, Ui};
use ferric_core::timestamp::{self, Precision};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
struct TimestampDraft {
    tz: String,
    ts_input: String,
    date_input: String,
}

pub struct TimestampTool {
    tz: chrono_tz::Tz,
    tz_filter: String,
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
    egui::Frame::none()
        .fill(theme.code_bg)
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
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
                ComboBox::from_id_salt("tz-combo")
                    .selected_text(self.tz.name())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.tz_filter)
                                .desired_width(f32::INFINITY)
                                .hint_text("搜索时区…"),
                        );
                        let f = self.tz_filter.to_lowercase();
                        // 全量列出（约 590 个），超长部分靠下拉内滚动，不截断。
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for z in chrono_tz::TZ_VARIANTS.iter().filter(|z| {
                                    f.is_empty() || z.name().to_lowercase().contains(&f)
                                }) {
                                    ui.selectable_value(&mut self.tz, *z, z.name());
                                }
                            });
                    });
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
