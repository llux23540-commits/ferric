//! 正则表达式测试 + 备忘单视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{Frame, Margin, RichText, ScrollArea, Ui};
use ferric_core::regex;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct RegexDraft {
    pattern: String,
    flags: String,
    text: String,
}

pub struct RegexTool {
    pattern: String,
    fg: bool,
    fi: bool,
    fm: bool,
    fs: bool,
    fx: bool,
    text: String,
}

impl Default for RegexTool {
    fn default() -> Self {
        Self {
            pattern: r"(\w+)@(\w+\.\w+)".to_owned(),
            fg: true,
            fi: false,
            fm: false,
            fs: false,
            fx: false,
            text: "联系我们：hi@ferric.dev 或 support@example.com，也可发送到 dev@ferric.io。"
                .to_owned(),
        }
    }
}

impl RegexTool {
    fn flags(&self) -> String {
        let mut s = String::new();
        if self.fg {
            s.push('g');
        }
        if self.fi {
            s.push('i');
        }
        if self.fm {
            s.push('m');
        }
        if self.fs {
            s.push('s');
        }
        if self.fx {
            s.push('x');
        }
        s
    }
}

impl Tool for RegexTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "regex",
            name: "正则表达式",
            group: "文本",
            desc: "实时测试正则，高亮匹配、查看分组，并附常用语法备忘单。",
            icon: icons::TERMINAL,
            keywords: &["regex", "正则", "regexp", "match"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        let flags = self.flags();
        let result = regex::find_all(&self.pattern, &flags, &self.text);

        // 模式输入行： /pattern/flags + 状态
        Frame::NONE
            .fill(theme.code_bg)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("/").monospace().size(16.0).color(theme.faint));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pattern)
                            .frame(egui::Frame::NONE)
                            .desired_width((ui.available_width() - 120.0).max(80.0))
                            .hint_text(r"正则表达式，如 (\w+)@(\w+\.\w+)")
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.label(RichText::new("/").monospace().size(16.0).color(theme.faint));
                    ui.label(
                        RichText::new(&flags)
                            .monospace()
                            .size(13.0)
                            .color(theme.accent_strong),
                    );
                });
            });
        ui.add_space(6.0);
        // 状态
        match &result {
            Ok(m) => widgets::status_line(ui, &theme, true, &format!("{} 处匹配", m.len())),
            Err(e) => widgets::status_line(ui, &theme, false, &format!("正则错误：{e}")),
        }
        ui.add_space(8.0);

        // 标志
        ui.horizontal_wrapped(|ui| {
            for (on, label) in [
                (&mut self.fg, "g 全局"),
                (&mut self.fi, "i 忽略大小写"),
                (&mut self.fm, "m 多行"),
                (&mut self.fs, "s 点匹配换行"),
                (&mut self.fx, "x 扩展"),
            ] {
                if widgets::pill_toggle(ui, &theme, *on, label) {
                    *on = !*on;
                }
            }
        });
        ui.add_space(10.0);

        // 测试文本 + 匹配结果
        ui.columns(2, |cols| {
            widgets::field_label(&mut cols[0], &theme, "测试文本");
            cols[0].add_space(4.0);
            widgets::code_area(&mut cols[0], "rx-text", &mut self.text, true, 8);

            widgets::field_label(&mut cols[1], &theme, "匹配结果 / 分组");
            cols[1].add_space(4.0);
            match &result {
                Ok(matches) if !matches.is_empty() => {
                    ScrollArea::vertical()
                        .id_salt("rx-matches")
                        .max_height(200.0)
                        .auto_shrink([false, false])
                        .show(&mut cols[1], |ui| {
                            for (i, m) in matches.iter().enumerate() {
                                widgets::card(ui, &theme, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("#{}", i + 1))
                                                .size(12.0)
                                                .strong()
                                                .color(theme.accent_strong),
                                        );
                                        ui.label(
                                            RichText::new(format!("at {}..{}", m.start, m.end))
                                                .size(11.0)
                                                .color(theme.faint)
                                                .monospace(),
                                        );
                                    });
                                    ui.label(
                                        RichText::new(&m.text)
                                            .monospace()
                                            .size(12.5)
                                            .color(theme.fg)
                                            .background_color(theme.accent_soft),
                                    );
                                    for (gi, g) in m.groups.iter().enumerate() {
                                        if let Some(g) = g {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(format!("组 {}", gi + 1))
                                                        .size(11.0)
                                                        .color(theme.muted),
                                                );
                                                ui.label(
                                                    RichText::new(g)
                                                        .monospace()
                                                        .size(12.0)
                                                        .color(theme.fg_soft),
                                                );
                                            });
                                        }
                                    }
                                });
                                ui.add_space(6.0);
                            }
                        });
                }
                Ok(_) => {
                    cols[1].label(RichText::new("无匹配").size(12.0).color(theme.faint));
                }
                Err(_) => {
                    cols[1].label(RichText::new("正则无效").size(12.0).color(theme.danger));
                }
            }
        });

        ui.add_space(18.0);
        cheat_sheet(ui, &theme);
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&RegexDraft {
            pattern: self.pattern.clone(),
            flags: self.flags(),
            text: self.text.clone(),
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<RegexDraft>(data) {
            self.pattern = d.pattern;
            self.text = d.text;
            self.fg = d.flags.contains('g');
            self.fi = d.flags.contains('i');
            self.fm = d.flags.contains('m');
            self.fs = d.flags.contains('s');
            self.fx = d.flags.contains('x');
        }
    }
}

const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "普通字符",
        &[
            (r". 或 [^\n\r]", "除换行符或回车符之外的任何字符"),
            ("[A-Za-z]", "字母"),
            ("[a-z]", "小写字母"),
            ("[A-Z]", "大写字母"),
            (r"\d 或 [0-9]", "数字"),
            (r"\D 或 [^0-9]", "非数字"),
            ("_", "下划线"),
            (r"\w", "字母、数字或下划线"),
            (r"\W", r"\w 的反义"),
            (r"\S", r"\s 的反义"),
        ],
    ),
    (
        "空白字符",
        &[
            (r"\t", "制表符"),
            (r"\n", "换行符"),
            (r"\r", "回车符"),
            (r"\s", "空格、制表符、换行符或回车符"),
        ],
    ),
    (
        "字符集",
        &[
            ("[xyz]", "x、y 或 z"),
            ("[^xyz]", "既不是 x，也不是 y，也不是 z"),
            ("[1-3]", "1、2 或 3"),
            ("[^1-3]", "既不是 1、2，也不是 3"),
        ],
    ),
    (
        "量词",
        &[
            ("{2}", "正好 2 次"),
            ("{2,}", "至少 2 次"),
            ("{2,7}", "至少 2 次但不多于 7 次"),
            ("*", "0 次或多次"),
            ("+", "1 次或多次"),
            ("?", "0 次或 1 次"),
        ],
    ),
    (
        "边界",
        &[
            ("^", "字符串开头"),
            ("$", "字符串结尾"),
            (r"\b", "单词边界"),
        ],
    ),
    (
        "匹配（分支与断言）",
        &[
            ("foo|bar", "匹配 foo 或 bar"),
            ("foo(?=bar)", "若 foo 后跟 bar，则匹配 foo"),
            ("foo(?!bar)", "若 foo 后不跟 bar，则匹配 foo"),
        ],
    ),
];

fn cheat_sheet(ui: &mut Ui, theme: &crate::theme::Theme) {
    widgets::field_label(ui, theme, "正则表达式备忘单");
    ui.add_space(8.0);
    ui.columns(2, |cols| {
        for (idx, (title, rows)) in SECTIONS.iter().enumerate() {
            let col = &mut cols[idx % 2];
            widgets::card(col, theme, |ui| {
                ui.label(
                    RichText::new(*title)
                        .family(egui::FontFamily::Name(crate::fonts::UI_SEMIBOLD.into()))
                        .size(13.0)
                        .color(theme.accent_strong),
                );
                ui.add_space(6.0);
                for (expr, desc) in rows.iter() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(*expr)
                                .monospace()
                                .size(12.5)
                                .color(theme.fg)
                                .background_color(theme.code_bg),
                        );
                        ui.label(RichText::new(*desc).size(12.0).color(theme.muted));
                    });
                }
            });
            col.add_space(10.0);
        }
    });
}
