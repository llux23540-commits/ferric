//! Ferric 桌面客户端入口。
//!
//! 这里只做一件事：**把窗口开出来，而且要开得起来**。
//! 渲染后端在不同机器上能不能用差别极大（无驱动的虚拟机、精简版 Windows、远程
//! 桌面……），所以启动是一个「按计划逐个尝试 + 记住哪个成功」的过程，
//! 而不是一次性的 `run_native`。计划怎么排、失败了怎么自愈，见 [`ferric_ui::launch`]。

// 发行版隐藏 Windows 控制台窗口。
// 代价是 stderr 没有任何去处 —— 启动失败必须落到日志文件 + 弹窗，
// 否则用户看到的就是「双击了没反应」。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use ferric_ui::launch;
use ferric_ui::{FerricApp, APP_NAME};

fn native_options() -> eframe::NativeOptions {
    // 窗口/任务栏图标（Windows 标题栏+任务栏、X11）。Wayland 不走这里 ——
    // 合成器按 app_id 找 .desktop 文件拿图标，见下面的 with_app_id。
    // macOS Dock 用的是 bundle 里的 icns（cargo-packager 打包时带入）。
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../icons/128x128@2x.png"))
        .expect("内嵌图标是构建期资源，坏了只能是资源本身被改坏");
    eframe::NativeOptions {
        // wgpu 后端。具体用 DX12 / Vulkan / GL 由 launch::plan 通过 WGPU_BACKEND
        // 决定（egui-wgpu 的默认适配器设置读的就是这个环境变量）。
        renderer: eframe::Renderer::Wgpu,
        // 首次在主屏居中打开（之后由 persist_window 记住用户调整）。
        centered: true,
        // 表面配置压到「低延迟」：GUI 本身没有 GPU 工作量，把呈现队列从默认的
        // 2 帧降到 1 帧。排队的帧越少，画面与实际状态越同步 —— 软件光栅化的机器上
        // 排队等于让一帧陈旧内容多停留一个刷新周期，看着就是闪。
        wgpu_options: eframe::WgpuConfiguration {
            surface: eframe::SurfaceConfig::LOW_LATENCY,
            ..Default::default()
        },
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
            // 同时决定 eframe 的状态目录（launch.json 也放在那儿）。
            .with_app_id(launch::APP_ID)
            .with_title(APP_NAME),
        ..Default::default()
    }
}

fn run_once() -> eframe::Result<()> {
    eframe::run_native(
        APP_NAME,
        native_options(),
        Box::new(|cc| Ok(Box::new(FerricApp::new(cc)))),
    )
}

fn main() -> eframe::Result<()> {
    // 选后端 + 落下「正在尝试」标记。winit 全进程只允许一次事件循环，
    // 所以这里**不能**失败了在同一进程里换个后端重来 —— 自愈是跨启动的，
    // 崩过的这次会被记下，下次启动自动轮到下一个。细节见 launch::plan。
    let mut cfg = launch::load();
    let backend = launch::begin(&mut cfg);
    launch::log(&format!("启动：渲染后端 = {}", backend.label()));

    match run_once() {
        Ok(()) => Ok(()),
        Err(err) => {
            let detail = err.to_string();
            launch::log(&format!("以 {} 启动失败：{detail}", backend.label()));
            // 已经出过帧的话就不是「打不开」，而是跑着跑着出的错。那种情况：
            // 既不弹「无法启动」（误导），也**不碰配置文件** —— 盘上那份已经被
            // mark_running 标成「这个后端是好的」，把内存里带着 pending 的旧快照
            // 存回去，等于让下次启动误以为上次崩在了启动路上。
            if !launch::is_running() {
                cfg.last_error = Some(detail.clone());
                // pending 保持原样：下次启动据此把这个后端排到最后，改用下一个。
                launch::save(&cfg);
                launch::fatal_dialog(&detail);
            }
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferric_ui::launch::{Backend, LaunchCfg};

    /// 计划里必须包含「自动」这一档，而且不能是空的 ——
    /// 空计划意味着这次启动一个后端都不会用。
    #[test]
    fn plan_is_never_empty() {
        let p = launch::plan(&LaunchCfg::default());
        assert!(!p.is_empty());
        assert!(p.contains(&Backend::Auto));
    }

    /// 默认情况（全新安装）就该走「自动」，让 wgpu 自己挑。
    #[test]
    fn a_fresh_install_starts_on_auto() {
        assert_eq!(
            launch::plan(&LaunchCfg::default()).first(),
            Some(&Backend::Auto)
        );
    }
}
