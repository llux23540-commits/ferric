//! 字体与图标字体注入。
//!
//! 内嵌设计字体：**Plus Jakarta Sans**（UI，含 Medium/SemiBold/Bold 命名族）、
//! **JetBrains Mono**（等宽）、**Lucide**（图标字体，见 [`crate::icons`]）。
//! 中文从系统字体加载并作为 UI / 等宽两族的回退。

use egui::{FontData, FontDefinitions, FontFamily};

// 编译期内嵌的设计字体（crates/ferric-ui/assets/fonts）。
const PJS_REGULAR: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-Regular.ttf");
const PJS_MEDIUM: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-Medium.ttf");
const PJS_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-SemiBold.ttf");
const PJS_BOLD: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-Bold.ttf");
const JBM_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const JBM_MEDIUM: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");
const LUCIDE: &[u8] = include_bytes!("../assets/fonts/lucide.ttf");

/// 命名字体族（配合 `RichText::family(FontFamily::Name(...))` 使用）。
pub const UI_MEDIUM: &str = "ui-medium";
pub const UI_SEMIBOLD: &str = "ui-semibold";
pub const UI_BOLD: &str = "ui-bold";
pub const MONO_MEDIUM: &str = "mono-medium";
pub const LUCIDE_FAMILY: &str = "lucide";

/// 各平台常见的中文字体候选路径（按优先级）。
#[cfg(target_os = "windows")]
const CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑
    r"C:\Windows\Fonts\msyh.ttf",
    r"C:\Windows\Fonts\simhei.ttf", // 黑体
    r"C:\Windows\Fonts\simsun.ttc", // 宋体
];

#[cfg(target_os = "macos")]
const CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
];

#[cfg(all(unix, not(target_os = "macos")))]
const CANDIDATES: &[&str] = &[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
];

/// 注册全部字体族。
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    fonts
        .font_data
        .insert("pjs".into(), FontData::from_static(PJS_REGULAR));
    fonts
        .font_data
        .insert("pjs-med".into(), FontData::from_static(PJS_MEDIUM));
    fonts
        .font_data
        .insert("pjs-semi".into(), FontData::from_static(PJS_SEMIBOLD));
    fonts
        .font_data
        .insert("pjs-bold".into(), FontData::from_static(PJS_BOLD));
    fonts
        .font_data
        .insert("jbm".into(), FontData::from_static(JBM_REGULAR));
    fonts
        .font_data
        .insert("jbm-med".into(), FontData::from_static(JBM_MEDIUM));
    fonts
        .font_data
        .insert("lucide".into(), FontData::from_static(LUCIDE));

    // 系统中文字体作为回退。
    let has_cjk = match load_first_cjk() {
        Some(bytes) => {
            fonts
                .font_data
                .insert("cjk".into(), FontData::from_owned(bytes));
            true
        }
        None => false,
    };

    // 主族：把设计字体前置，中文其后，保留 egui 默认（含 emoji 回退）在末尾。
    front(&mut fonts, FontFamily::Proportional, "pjs", has_cjk);
    front(&mut fonts, FontFamily::Monospace, "jbm", has_cjk);

    // 命名族：粗细变体与图标。
    named(&mut fonts, UI_MEDIUM, "pjs-med", has_cjk);
    named(&mut fonts, UI_SEMIBOLD, "pjs-semi", has_cjk);
    named(&mut fonts, UI_BOLD, "pjs-bold", has_cjk);
    named(&mut fonts, MONO_MEDIUM, "jbm-med", has_cjk);
    fonts
        .families
        .insert(FontFamily::Name(LUCIDE_FAMILY.into()), vec!["lucide".into()]);

    ctx.set_fonts(fonts);
}

/// 把 `primary`（+可选 cjk）前置到已有族的最前，保留原有回退于末尾。
fn front(fonts: &mut FontDefinitions, fam: FontFamily, primary: &str, has_cjk: bool) {
    let base = fonts.families.remove(&fam).unwrap_or_default();
    let mut v = vec![primary.to_string()];
    if has_cjk {
        v.push("cjk".to_string());
    }
    v.extend(base);
    fonts.families.insert(fam, v);
}

/// 创建一个命名族：`primary`（+可选 cjk 回退）。
fn named(fonts: &mut FontDefinitions, name: &str, primary: &str, has_cjk: bool) {
    let mut v = vec![primary.to_string()];
    if has_cjk {
        v.push("cjk".to_string());
    }
    fonts
        .families
        .insert(FontFamily::Name(name.into()), v);
}

fn load_first_cjk() -> Option<Vec<u8>> {
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(bytes);
        }
    }
    None
}
