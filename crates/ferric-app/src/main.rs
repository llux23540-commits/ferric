//! Ferric 桌面客户端入口。
//!
//! 默认用 OpenGL（glow）渲染；打不开就改用 CPU 渲染并记住，见 [`ferric_ui::launch`]。

// 发行版隐藏 Windows 控制台窗口。
// 代价是 stderr 没有任何去处 —— 启动失败必须落到日志文件 + 弹窗，
// 否则用户看到的就是「双击了没反应」。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use ferric_soft_render as soft;
use ferric_ui::launch;
use ferric_ui::launch::Backend;
use ferric_ui::{FerricApp, APP_NAME};

fn native_options() -> eframe::NativeOptions {
    // 窗口/任务栏图标（Windows 标题栏+任务栏、X11）。Wayland 不走这里 ——
    // 合成器按 app_id 找 .desktop 文件拿图标，见下面的 with_app_id。
    // macOS Dock 用的是 bundle 里的 icns（cargo-packager 打包时带入）。
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../icons/128x128@2x.png"))
        .expect("内嵌图标是构建期资源，坏了只能是资源本身被改坏");
    eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        // 首次在主屏居中打开（之后由 persist_window 记住用户调整）。
        centered: true,
        viewport: egui::ViewportBuilder::default()
            // 首次启动的默认尺寸；之后由 eframe 记住用户自己调过的大小。
            // 默认 1280×800（比早期的 1560×980 收敛）：虚拟机 / 远程桌面的软渲染
            // 没有 GPU 显存，窗口物理像素直接吃进程内存，尺寸每大一圈、内存与
            // CPU 都跟着涨。有 GPU 的机器不受影响 —— 想开大随时拖大，eframe 会记住。
            .with_inner_size([1280.0, 800.0])
            // 最小尺寸放到 640×420：软件光栅化（无 GPU 的虚拟机）下，每帧开销与
            // **窗口物理像素数成正比**——实测把窗口从 1320×840 拖到约一半边长，交互
            // CPU 直接降到 ~1/4（4× 提速，≈4fps→≈16fps）。这是这类环境里唯一真正
            // 有效的软件侧手段（关动画/阴影/羽化都只是杯水车薪，实测无感）。原来的
            // 下限 1000×640 卡住了用户往这个方向自救；调低后可拖到真正流畅的尺寸，
            // 且 eframe 会记住。有 GPU 的机器不受影响——他们没有拖小的动机。
            .with_min_inner_size([640.0, 420.0])
            .with_resizable(true)
            .with_decorations(false) // 自绘标题栏（缩放由 chrome::handle_resize 手动处理）
            // 不透明窗口：软件 OpenGL 与 CPU 渲染都不一定支持透明表面。
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

fn run_once(backend: Backend) -> Result<(), String> {
    if backend == Backend::Soft {
        // 纯 CPU 渲染：不建任何显卡上下文。窗口外观沿用 native_options 的 viewport 配置。
        return soft::run_soft(
            soft::SoftOptions {
                viewport: native_options().viewport,
                app_id: Some(launch::APP_ID.to_owned()),
            },
            Box::new(
                |ctx: &egui::Context, storage: Option<Box<dyn eframe::Storage>>| {
                    Ok::<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>>(Box::new(
                        FerricApp::new_soft(ctx, storage),
                    ))
                },
            ),
        );
    }
    eframe::run_native(
        APP_NAME,
        native_options(),
        Box::new(|cc| Ok(Box::new(FerricApp::new(cc)))),
    )
    .map_err(|e| e.to_string())
}

/// 把纯字符串的启动失败包装成 `eframe::Error`（eframe 没有 `From<String>`）。
#[derive(Debug)]
struct LaunchError(String);

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LaunchError {}

fn main() -> eframe::Result<()> {
    let mut cfg = launch::load();
    let backend = launch::begin(&mut cfg);
    launch::log(&format!("启动：渲染方式 = {}", backend.label()));

    let Err(detail) = run_once(backend) else {
        return Ok(());
    };
    launch::log(&format!("以 {} 启动失败：{detail}", backend.label()));
    // 已经出过帧的话就不是「打不开」，而是跑着跑着出的错：不弹窗、不改配置。
    if !launch::is_running() {
        // OpenGL 打不开 → 改用 CPU 渲染，重新拉起自己（winit 全进程只允许一次
        // 事件循环，同一进程里没法换个渲染器重来）。环境变量强制了渲染方式时
        // 不重启 —— 子进程会继承它，重启只会原样再失败一次，无限循环。
        let forced = std::env::var_os("FERRIC_RENDERER").is_some();
        let relaunched = backend == Backend::Glow && !forced && {
            launch::fall_back_to_soft(&mut cfg, &detail);
            launch::relaunch().is_ok()
        };
        if relaunched {
            return Ok(());
        }
        launch::fatal_dialog(&detail);
    }
    Err(eframe::Error::AppCreation(Box::new(LaunchError(detail))))
}
