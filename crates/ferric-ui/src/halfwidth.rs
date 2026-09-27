//! 键入时把全角字母 / 数字 / 全角空格归一成半角。
//!
//! 输入法处于「全角」状态（Shift+空格 是最常见的误触）时，敲出来的是 ａｂｃ１２３
//! （U+FF41… / U+FF11…）。这些字形只画半格、却按一个汉字宽排版，于是每个字母后面都
//! 空出半格 —— 看上去就是「光标离文字差半个字」；放进 JSON / SQL / 正则里还直接是语法错误。
//!
//! 只动字母、数字与全角空格：，。：（）！ 这些全角标点是正常的中文内容，保留原样。
//! 只处理键入与输入法事件（`Text` / `Ime`），粘贴与已有正文不动。

use egui::{Event, ImeEvent};

/// 在所有控件读输入之前调用：原地改写本帧的键入 / 输入法事件。
///
/// 每个视口各有一份输入，独立视口（设置窗）要在它自己的回调里再调一次。
pub fn normalize_typed_input(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        for ev in &mut i.events {
            match ev {
                Event::Text(t)
                | Event::Ime(ImeEvent::Commit(t))
                // 预编辑区间按字符计数，逐字符一对一替换不会让它失效
                | Event::Ime(ImeEvent::Preedit { text: t, .. }) => to_halfwidth(t),
                _ => {}
            }
        }
    });
}

fn to_halfwidth(s: &mut String) {
    // 绝大多数事件里没有全角字符：不分配，原样返回
    if s.chars().all(|c| halfwidth(c) == c) {
        return;
    }
    *s = s.chars().map(halfwidth).collect();
}

fn halfwidth(c: char) -> char {
    match c {
        // 全角区与 ASCII 相差固定偏移 0xFEE0
        '０'..='９' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ' => {
            char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
        }
        '\u{3000}' => ' ',
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunUiExt;

    #[test]
    fn converts_only_letters_digits_and_ideographic_space() {
        let mut s = "ａｚＡＺ０９\u{3000}，。：（）！＠［｀｛中文abc".to_owned();
        to_halfwidth(&mut s);
        assert_eq!(s, "azAZ09 ，。：（）！＠［｀｛中文abc");
    }

    #[test]
    fn rewrites_text_and_ime_events_before_widgets_read_them() {
        let ctx = egui::Context::default();
        let events = vec![
            Event::Text("ｊｓｏｎ，".to_owned()),
            Event::Ime(ImeEvent::Preedit {
                text: "ｆｏｏ".to_owned(),
                active_range_chars: Some(1..3),
            }),
            Event::Ime(ImeEvent::Commit("１２\u{3000}中".to_owned())),
            Event::Paste("ａｂｃ".to_owned()),
        ];
        let mut seen = Vec::new();
        ctx.run_ui_cleared(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                normalize_typed_input(ui.ctx());
                seen = ui.input(|i| i.events.clone());
            },
        );
        assert_eq!(
            seen,
            vec![
                Event::Text("json，".to_owned()),
                Event::Ime(ImeEvent::Preedit {
                    text: "foo".to_owned(),
                    active_range_chars: Some(1..3),
                }),
                Event::Ime(ImeEvent::Commit("12 中".to_owned())),
                // 粘贴保持原样：那是用户明确要放进来的内容
                Event::Paste("ａｂｃ".to_owned()),
            ]
        );
    }
}
