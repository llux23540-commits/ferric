//! UUID 生成器视图（v4 / v7 / v5 命名 / v6）。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{ComboBox, RichText, Ui};
use ferric_core::idgen::{self, IdKind, Namespace, Opts};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct UuidDraft {
    kind: IdKind,
    count: i64,
    namespace: Namespace,
    custom_ns: String,
    name: String,
    upper: bool,
    nohyphen: bool,
    as_json: bool,
}

struct HistEntry {
    label: String,
    body: String,
}

pub struct UuidTool {
    kind: IdKind,
    count: i64,
    namespace: Namespace,
    custom_ns: String,
    name: String,
    upper: bool,
    nohyphen: bool,
    as_json: bool,
    output: String,
    ok: bool,
    status: String,
    history: Vec<HistEntry>,
    counter: u32,
}

impl Default for UuidTool {
    fn default() -> Self {
        let mut t = Self {
            kind: IdKind::UuidV4,
            count: 10,
            namespace: Namespace::Dns,
            custom_ns: String::new(),
            name: "example.com".to_owned(),
            upper: false,
            nohyphen: false,
            as_json: false,
            output: String::new(),
            ok: true,
            status: "就绪".to_owned(),
            history: Vec::new(),
            counter: 0,
        };
        t.regen();
        t
    }
}

impl UuidTool {
    fn opts(&self) -> Opts<'_> {
        Opts {
            kind: self.kind,
            count: self.count.clamp(1, 1000) as usize,
            namespace: self.namespace,
            custom_ns: &self.custom_ns,
            name: &self.name,
            upper: self.upper,
            nohyphen: self.nohyphen,
            as_json: self.as_json,
        }
    }

    fn regen(&mut self) {
        // 生成失败（如无效自定义命名空间）时不覆盖输出、不记历史，只报状态。
        match idgen::generate(&self.opts()) {
            Ok(items) => {
                self.output = if self.as_json {
                    serde_json::to_string_pretty(&items).unwrap_or_default()
                } else {
                    items.join("\n")
                };
                self.ok = true;
                self.status = format!("已生成 {} 条", items.len());
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("生成失败：{e}");
                return;
            }
        }
        self.counter = self.counter.wrapping_add(1);
        // 记入历史：history[0] 是当前这次，其后保留最近 3 次
        let label = format!(
            "{} · {} 个 · #{}",
            self.kind.label(),
            self.count.clamp(1, 1000),
            self.counter
        );
        self.history.insert(
            0,
            HistEntry {
                label,
                body: self.output.clone(),
            },
        );
        self.history.truncate(4);
    }
}

impl Tool for UuidTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "uuid",
            name: "UUID 生成器",
            group: "生成",
            desc: "生成 UUID —— 支持 v4（随机）、v7 / v6（时间有序）、v5（命名空间 + 名称）。",
            icon: crate::icons::CREDIT_CARD,
            keywords: &["uuid", "guid", "v4", "v5", "v7", "生成", "标识符"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 版本
        widgets::field_label(ui, &theme, "版本");
        ui.add_space(4.0);
        let labels: Vec<&str> = IdKind::ALL.iter().map(|k| k.label()).collect();
        let cur = IdKind::ALL
            .iter()
            .position(|k| *k == self.kind)
            .unwrap_or(0);
        ui.horizontal(|ui| {
            if let Some(n) = widgets::seg(ui, &theme, &labels, cur) {
                self.kind = IdKind::ALL[n];
                self.regen();
            }
        });
        ui.add_space(10.0);

        // 数量
        widgets::field_label(ui, &theme, "数量");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if widgets::num_field(ui, &theme, &mut self.count, 1, 1000, 1) {
                self.regen();
            }
        });
        ui.add_space(10.0);

        // 命名空间（仅 v5）
        if self.kind.is_named() {
            widgets::field_label(ui, &theme, "命名");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ComboBox::from_id_salt("uuid-ns")
                    .selected_text(self.namespace.label())
                    .show_ui(ui, |ui| {
                        for ns in Namespace::ALL {
                            if ui
                                .selectable_value(&mut self.namespace, ns, ns.label())
                                .clicked()
                            {
                                self.regen();
                            }
                        }
                    });
                if self.namespace == Namespace::Custom
                    && ui
                        .add(
                            egui::TextEdit::singleline(&mut self.custom_ns)
                                .desired_width(260.0)
                                .hint_text("自定义命名空间 UUID"),
                        )
                        .changed()
                {
                    self.regen();
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                widgets::field_label(ui, &theme, "名称");
                ui.add_space(6.0);
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.name)
                            .desired_width(300.0)
                            .hint_text("名称，如 example.com"),
                    )
                    .changed()
                {
                    self.regen();
                }
            });
            ui.add_space(10.0);
        }

        // 格式
        widgets::field_label(ui, &theme, "格式");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let fmt = if self.as_json { 1 } else { 0 };
            if let Some(n) = widgets::seg(ui, &theme, &["Raw", "JSON"], fmt) {
                self.as_json = n == 1;
                self.regen();
            }
            ui.add_space(10.0);
            if widgets::pill_toggle(ui, &theme, self.upper, "大写") {
                self.upper = !self.upper;
                self.regen();
            }
            if widgets::pill_toggle(ui, &theme, self.nohyphen, "去连字符") {
                self.nohyphen = !self.nohyphen;
                self.regen();
            }
        });
        ui.add_space(12.0);

        // 操作 + 输出
        ui.horizontal(|ui| {
            if widgets::primary_icon(ui, &theme, icons::REFRESH_CW, "刷新").clicked() {
                self.regen();
            }
            if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制").clicked() {
                let out = self.output.clone();
                shared.copy(ui.ctx(), out);
            }
            ui.add_space(6.0);
            widgets::status_line(ui, &theme, self.ok, &self.status);
        });
        ui.add_space(8.0);
        ui.add_space(4.0);
        widgets::code_area(ui, "uuid-out", &mut self.output, false, 10);

        // 历史
        ui.add_space(16.0);
        widgets::field_label(ui, &theme, "执行记录（保留最近 3 次）");
        ui.add_space(6.0);
        if self.history.len() <= 1 {
            ui.label(
                RichText::new("暂无记录，点「刷新」生成一次即可记录")
                    .size(12.0)
                    .color(theme.faint),
            );
        } else {
            let hist: Vec<(String, String)> = self
                .history
                .iter()
                .skip(1)
                .map(|h| (h.label.clone(), h.body.clone()))
                .collect();
            for (label, body) in hist {
                widgets::card(ui, &theme, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&label)
                                .size(11.5)
                                .color(theme.muted)
                                .monospace(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制")
                                .clicked()
                            {
                                shared.copy(ui.ctx(), body.clone());
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let preview: String = body.lines().take(3).collect::<Vec<_>>().join("\n");
                    ui.label(
                        RichText::new(preview)
                            .size(12.0)
                            .monospace()
                            .color(theme.fg_soft),
                    );
                });
                ui.add_space(8.0);
            }
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&UuidDraft {
            kind: self.kind,
            count: self.count,
            namespace: self.namespace,
            custom_ns: self.custom_ns.clone(),
            name: self.name.clone(),
            upper: self.upper,
            nohyphen: self.nohyphen,
            as_json: self.as_json,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<UuidDraft>(data) {
            self.kind = d.kind;
            self.count = d.count.clamp(1, 1000);
            self.namespace = d.namespace;
            self.custom_ns = d.custom_ns;
            self.name = d.name;
            self.upper = d.upper;
            self.nohyphen = d.nohyphen;
            self.as_json = d.as_json;
            self.history.clear();
            self.regen();
        }
    }
}
