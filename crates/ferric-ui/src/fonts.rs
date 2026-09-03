//! 字体与图标字体注入。
//!
//! 内嵌设计字体：**Plus Jakarta Sans**（UI，含 Medium/SemiBold/Bold 命名族）、
//! **JetBrains Mono**（等宽）、**Lucide**（图标字体，见 [`crate::icons`]）。
//! 中文从系统字体加载并作为 UI / 等宽两族的回退。

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

// 编译期内嵌的设计字体（crates/ferric-ui/assets/fonts）。
pub const PJS_REGULAR: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-Regular.ttf");
pub const PJS_MEDIUM: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-Medium.ttf");
pub const PJS_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-SemiBold.ttf");
pub const PJS_BOLD: &[u8] = include_bytes!("../assets/fonts/PlusJakartaSans-Bold.ttf");
pub const JBM_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
pub const JBM_MEDIUM: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");
pub const LUCIDE: &[u8] = include_bytes!("../assets/fonts/lucide.ttf");

/// 命名字体族（配合 `RichText::family(FontFamily::Name(...))` 使用）。
pub const UI_MEDIUM: &str = "ui-medium";
pub const UI_SEMIBOLD: &str = "ui-semibold";
pub const UI_BOLD: &str = "ui-bold";
pub const MONO_MEDIUM: &str = "mono-medium";
pub const LUCIDE_FAMILY: &str = "lucide";

/// 各平台常见的中文字体候选路径（按优先级）。
#[cfg(target_os = "windows")]
const CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc", // 微软雅黑
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

/// 注册全部字体族。返回**是否找到了中文字体** —— 没找到的话界面上的中文会是
/// 一片方块（本应用的文案几乎全是中文，等于整个界面废掉），调用方应当提示用户。
pub fn install_fonts(ctx: &egui::Context) -> bool {
    let mut fonts = FontDefinitions::default();

    fonts
        .font_data
        .insert("pjs".into(), Arc::new(FontData::from_static(PJS_REGULAR)));
    fonts.font_data.insert(
        "pjs-med".into(),
        Arc::new(FontData::from_static(PJS_MEDIUM)),
    );
    fonts.font_data.insert(
        "pjs-semi".into(),
        Arc::new(FontData::from_static(PJS_SEMIBOLD)),
    );
    fonts
        .font_data
        .insert("pjs-bold".into(), Arc::new(FontData::from_static(PJS_BOLD)));
    fonts
        .font_data
        .insert("jbm".into(), Arc::new(FontData::from_static(JBM_REGULAR)));
    fonts.font_data.insert(
        "jbm-med".into(),
        Arc::new(FontData::from_static(JBM_MEDIUM)),
    );
    fonts
        .font_data
        .insert("lucide".into(), Arc::new(FontData::from_static(LUCIDE)));

    // 系统中文字体作为回退。
    let has_cjk = match load_first_cjk() {
        Some(bytes) => {
            fonts
                .font_data
                .insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
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
    fonts.families.insert(
        FontFamily::Name(LUCIDE_FAMILY.into()),
        vec!["lucide".into()],
    );

    ctx.set_fonts(fonts);
    record_cjk_bytes(if has_cjk { cjk_loaded_bytes() } else { 0 });
    has_cjk
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
    fonts.families.insert(FontFamily::Name(name.into()), v);
}

fn load_first_cjk() -> Option<Vec<u8>> {
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(bytes);
        }
    }
    scan_for_cjk()
}

/// 启动后 [`install_fonts`] 写入的 CJK 字节数；0 表示未找到 CJK 字体。
/// 用 `AtomicUsize` 是为了 `startup_diag` 在 const 上下文外能读；只写一次。
static CJK_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn cjk_loaded_bytes() -> usize {
    CJK_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

fn record_cjk_bytes(n: usize) {
    CJK_BYTES.store(n, std::sync::atomic::Ordering::Relaxed);
}

/// 启动诊断用：返回当前 `install_fonts` 实际拿到的 CJK 字体字节数。
/// 0 = 系统没找到中文字体（界面会显示方块，看一眼就懂）。
pub fn cjk_bytes() -> usize {
    cjk_loaded_bytes()
}

/// 启动诊断用：内嵌设计字体 + Lucide 图标字体的字节总和。
/// 这部分**一定**加载（编译期 `include_bytes!`），跟系统无关，
/// 是「确定性的下限」—— CJK 是「系统相关的不确定项」。
pub fn embedded_bytes() -> usize {
    PJS_REGULAR.len()
        + PJS_MEDIUM.len()
        + PJS_SEMIBOLD.len()
        + PJS_BOLD.len()
        + JBM_REGULAR.len()
        + JBM_MEDIUM.len()
        + LUCIDE.len()
}

/// 硬编码路径全落空之后再扫一遍常见位置。
///
/// Windows 上写死 `C:\Windows\Fonts` 会在三种真实情况下失手：系统不装在 C 盘、
/// 用户自己装的字体（无管理员权限时装到 `%LOCALAPPDATA%`）、以及精简版 / 英文版
/// 系统（微软雅黑压根没预装，用户手动装了 Noto / 思源）。这里按优先级在两个目录里
/// 逐个找，找到哪个用哪个。
#[cfg(target_os = "windows")]
fn scan_for_cjk() -> Option<Vec<u8>> {
    use std::path::PathBuf;

    // 优先级：雅黑 → 常见系统字体 → 用户可能自己装的开源中文字体
    const FILES: &[&str] = &[
        "msyh.ttc",
        "msyh.ttf",
        "msyhl.ttc",
        "msyhbd.ttc",
        "simhei.ttf",
        "simsun.ttc",
        "simkai.ttf",
        "simfang.ttf",
        "Deng.ttf",
        "msjh.ttc", // 微软正黑（繁体系统）
        "mingliu.ttc",
        "NotoSansSC-Regular.otf",
        "NotoSansCJKsc-Regular.otf",
        "SourceHanSansSC-Regular.otf",
        "SarasaGothicSC-Regular.ttf",
    ];

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(win) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(win).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(
            PathBuf::from(local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    for file in FILES {
        for dir in &dirs {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                return Some(bytes);
            }
        }
    }
    None
}

/// 非 Windows 平台：常见发行版路径已经写在 `CANDIDATES` 里，再扫一遍
/// fontconfig 的目录收益不大（缺字体时用户装一个包就好），这里不做额外事。
#[cfg(not(target_os = "windows"))]
fn scan_for_cjk() -> Option<Vec<u8>> {
    None
}
