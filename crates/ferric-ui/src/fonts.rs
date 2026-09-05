//! 字体（Slint 版）。
//!
//! 设计字体（Plus Jakarta Sans / JetBrains Mono / Lucide）由 **`.slint` 里的
//! `import "…ttf";`** 打进二进制并在运行期注册（见 `ui/theme.slint` 头部注释）。
//! 不是靠这里的 Rust 代码 —— Slint 没有公开的「注册字体」API。
//!
//! 运行期这里只做一件事：**看系统里有没有中文字体**。CJK 不能内嵌（几十 MB、
//! 各平台不同），由 Slint 的 `software-renderer-systemfonts` 自己枚举并回退；
//! 一个都没有时界面全是方块，等于整个界面废掉，所以要提示用户。

/// 内嵌设计字体的字节数（编译期常量）。
///
/// 只取 `.len()`：字节本身由 Slint 从 `.slint` 的 import 打进二进制，这里再
/// `include_bytes!` 一份就是把同样的 ~2MB 复制两遍。常量求值只留下长度。
const PJS_REGULAR_LEN: usize = include_bytes!("../assets/fonts/PlusJakartaSans-Regular.ttf").len();
const PJS_MEDIUM_LEN: usize = include_bytes!("../assets/fonts/PlusJakartaSans-Medium.ttf").len();
const PJS_SEMIBOLD_LEN: usize =
    include_bytes!("../assets/fonts/PlusJakartaSans-SemiBold.ttf").len();
const PJS_BOLD_LEN: usize = include_bytes!("../assets/fonts/PlusJakartaSans-Bold.ttf").len();
const JBM_REGULAR_LEN: usize = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").len();
const JBM_MEDIUM_LEN: usize = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf").len();
const LUCIDE_LEN: usize = include_bytes!("../assets/fonts/lucide.ttf").len();

/// 命名字体族。与 `ui/theme.slint` 的 `font-*` 属性一一对应，取值是字体
/// name 表里的 family（**不是文件名**）—— 写在 Rust 里是为了让「族名从哪来」
/// 有一个单一出处；改一边必须改另一边。
#[allow(dead_code)]
pub const UI_MEDIUM: &str = "Plus Jakarta Sans Medium";
#[allow(dead_code)]
pub const UI_SEMIBOLD: &str = "Plus Jakarta Sans SemiBold";
#[allow(dead_code)]
pub const UI_BOLD: &str = "Plus Jakarta Sans";
#[allow(dead_code)]
pub const MONO_MEDIUM: &str = "JetBrains Mono Medium";
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

/// 探测系统是否有中文字体。返回 `false` 时界面上的中文会是一片方块
///（本应用文案几乎全是中文，等于整个界面废掉），调用方应当提示用户。
///
/// # 为什么这里不再「注册」字体
///
/// egui 时代必须自己把 CJK 字体读进内存、塞进 `FontDefinitions` 的回退链
///（几十 MB 的 `Vec<u8>` 常驻，`mem.rs` 里那个 `fonts_src_bytes` 就是它）。
///
/// Slint 开了 `software-renderer-systemfonts` feature 之后，**系统字体由
/// Slint 自己通过 fontdb 枚举并按需回退** —— 我们不需要（也没有公开 API）
/// 手动注册。于是那几十 MB 的常驻拷贝直接消失，这是迁移顺手拿到的一笔内存收益。
///
/// 这个函数因此只剩一件事：**看一眼系统里到底有没有中文字体**，决定要不要
/// 提示用户。找到就记下字节数供 `mem.rs` 诊断用（磁盘大小，不再是常驻内存）。
pub fn install_fonts() -> bool {
    let Some(path) = find_cjk_path() else {
        record_cjk_bytes(0);
        crate::launch::log("未找到系统中文字体 —— 界面中文会显示为方块");
        return false;
    };
    let n = std::fs::metadata(&path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    record_cjk_bytes(n);
    crate::launch::log(&format!(
        "系统中文字体：{}（{n} 字节，由 Slint 按需回退，不常驻）",
        path.display()
    ));
    true
}

/// 找到第一个存在的中文字体**路径**（不读进内存 —— Slint 按路径注册，
/// 省掉一次几十 MB 的拷贝，这本身就是迁移顺手拿到的内存收益）。
fn find_cjk_path() -> Option<std::path::PathBuf> {
    for p in CANDIDATES {
        let path = std::path::Path::new(p);
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    scan_for_cjk_path()
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
/// 这部分**一定**加载（编译期打进二进制），跟系统无关，
/// 是「确定性的下限」—— CJK 是「系统相关的不确定项」。
pub fn embedded_bytes() -> usize {
    PJS_REGULAR_LEN
        + PJS_MEDIUM_LEN
        + PJS_SEMIBOLD_LEN
        + PJS_BOLD_LEN
        + JBM_REGULAR_LEN
        + JBM_MEDIUM_LEN
        + LUCIDE_LEN
}

/// 硬编码路径全落空之后再扫一遍常见位置。
///
/// Windows 上写死 `C:\Windows\Fonts` 会在三种真实情况下失手：系统不装在 C 盘、
/// 用户自己装的字体（无管理员权限时装到 `%LOCALAPPDATA%`）、以及精简版 / 英文版
/// 系统（微软雅黑压根没预装，用户手动装了 Noto / 思源）。这里按优先级在两个目录里
/// 逐个找，找到哪个用哪个。
#[cfg(target_os = "windows")]
fn scan_for_cjk_path() -> Option<std::path::PathBuf> {
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
            let p = dir.join(file);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 非 Windows 平台：常见发行版路径已经写在 `CANDIDATES` 里，再扫一遍
/// fontconfig 的目录收益不大（缺字体时用户装一个包就好），这里不做额外事。
#[cfg(not(target_os = "windows"))]
fn scan_for_cjk_path() -> Option<std::path::PathBuf> {
    None
}
