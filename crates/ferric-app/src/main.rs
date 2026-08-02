//! Ferric 桌面客户端入口。

// 发行版隐藏 Windows 控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use ferric_ui::{FerricApp, APP_NAME};

fn main() -> eframe::Result<()> {
    // 窗口/任务栏图标（Windows 标题栏+任务栏、X11）。Wayland 不走这里 ——
    // 合成器按 app_id 找 .desktop 文件拿图标，见下面的 with_app_id。
    // macOS Dock 用的是 bundle 里的 icns（cargo-packager 打包时带入）。
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../icons/128x128@2x.png"))
        .expect("内嵌图标是构建期资源，坏了只能是资源本身被改坏");
    let options = eframe::NativeOptions {
        // wgpu 后端。Windows 走 DX12（见 ferric-app 的 wgpu 依赖显式启用 dx12 feature，
        // 否则在无 Vulkan 驱动、GL 仅 1.1 的环境——如 QEMU 虚拟机的 WARP 软件渲染——找不到适配器）。
        renderer: eframe::Renderer::Wgpu,
        // 首次在主屏居中打开（之后由 persist_window 记住用户调整）。
        centered: true,
        viewport: egui::ViewportBuilder::default()
            // 对齐 index.html 的 .app 卡片尺寸；可自由缩放并记住。
            .with_inner_size([1320.0, 840.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_resizable(true)
            .with_decorations(false) // 自绘标题栏（缩放由 chrome::handle_resize 手动处理）
            // 关闭透明：WARP 软件光栅化器只支持 Opaque 表面，透明窗口会导致找不到适配器。
            // 有硬件 GPU 时可改回 true 获得圆角透明效果。
            .with_transparent(false)
            .with_icon(icon)
            // Wayland 的任务栏图标靠 app_id ↔ .desktop 文件名匹配；
            // 必须与打包产物的 desktop 文件名（= 二进制名 ferric）一致。
            .with_app_id("ferric")
            .with_title(APP_NAME),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(FerricApp::new(cc)))),
    )
}
