//! ferric-ui —— Slint 视图与应用外壳。
//!
//! 迁移说明（egui → Slint）：
//!
//! | egui 时代 | 现在 |
//! |---|---|
//! | `app.rs`（3609 行，状态 + 渲染） | `state.rs`（状态与桥接）+ `ui/*.slint`（视图） |
//! | `chrome.rs`（自绘标题栏） | `ui/app.slint` 的标题栏部分 |
//! | `widgets/mod.rs`（940 行组件） | `ui/widgets.slint` |
//! | `theme.rs`（egui Visuals 映射） | `ui/theme.slint`（global） |
//! | `eframe::Storage` | `persist.rs`（同目录同文件名同结构） |
//! | `ferric-soft-render`（自研 CPU 光栅化） | Slint `renderer-software` |
//!
//! 数据层（`net` / `github` / `source` / `market` / `updater` / `plugin_host` /
//! `release` / `mock` / `mem` / `launch`）**完全没动** —— 它们本来就不依赖 GUI。

mod editor;
mod editor_bridge;
mod fonts;
mod github;
mod icons;
pub mod launch;
mod market;
mod mem;
mod mock;
mod net;
mod persist;
mod plugin_host;
mod release;
mod source;
mod state;
mod tool;
mod updater;
mod views;

pub use state::{Shell, APP_NAME};

/// 编译期烘入的版本号（见 `build.rs`）。
pub fn version() -> &'static str {
    env!("FERRIC_VERSION")
}

/// 编译期烘入的构建号（git 提交数）。更新器比较新旧用。
pub fn build_number() -> &'static str {
    env!("FERRIC_BUILD_NUMBER")
}

/// 启动整个应用：选后端 → 装字体 → 建窗 → 跑事件循环。
///
/// 渲染后端固定走 Slint 的 software renderer：ferric 的承诺是「任何机器都能
/// 打开」，而软件渲染是唯一不依赖显卡驱动的路径。egui 时代那套「按计划逐个
/// 试 wgpu 后端 + 跨启动自愈」（`launch::plan` / `mark_running`）在这里退化
/// 成一条路 —— 但 `launch` 模块保留：它还管着 `startup.log` 与数据目录。
pub fn run() -> Result<(), slint::PlatformError> {
    use slint::ComponentHandle;

    launch::log("启动（Slint · software renderer）");
    launch::begin();

    slint::BackendSelector::new()
        .renderer_name("software".into())
        .select()?;

    let shell = Shell::new();
    let win = shell.build_window()?;

    // 系统中文字体探测（Slint 的 systemfonts 会自己做回退，这里只判断要不要提示）。
    if !fonts::install_fonts() {
        // 提示必须中英双语：这条消息本身也会是方块，英文那半句是用户唯一读得懂的。
        shell.state.borrow_mut().shared.toast_warn(
            "未找到中文字体，界面会显示为方块 / No CJK font found: \
             please install Microsoft YaHei or Noto Sans SC",
        );
        shell.sync_all(&win);
    }

    launch::mark_running();
    win.run()
}
