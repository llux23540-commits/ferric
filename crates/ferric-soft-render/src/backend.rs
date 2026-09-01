//! winit 事件循环 + egui 集成 + softbuffer 呈现。
//!
//! 这是软渲染后端的「外壳」：接管 eframe 的角色，把窗口事件喂给 egui，拿到这一帧
//! 的图形，交给 [`crate::raster::Renderer`] 光栅化，再用 softbuffer 贴到窗口。
//! 与 eframe 的区别只有一个 —— 渲染这一步完全在 CPU 上做，不创建任何 GPU 上下文。

use std::num::NonZeroU32;
use std::sync::Arc;

use egui::Context;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::window::{Window, WindowId};

// 让 `FileStorage::flush`（eframe::Storage 的方法）在 MutexGuard 上可调。
use eframe::Storage as _;

use crate::raster::Renderer;
use crate::storage::{FileStorage, SharedStorage};

/// 应用工厂：拿到 egui 上下文与持久化存储，返回一个 `eframe::App`。
/// 与 eframe 的 `AppCreator` 同构，只是把 storage 单独透传出来（软渲染后端自己
/// 也保留一份，好在关闭 / 重启时统一落盘）。
pub type AppCreator = Box<
    dyn FnOnce(
        &Context,
        Option<Box<dyn eframe::Storage>>,
    ) -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>>,
>;

#[derive(Debug)]
enum UserEvent {
    RequestRepaint,
}

/// 软渲染后端配置。窗口外观由 [`egui::ViewportBuilder`] 决定（与 eframe 一致）。
pub struct SoftOptions {
    pub viewport: egui::ViewportBuilder,
    /// 状态目录的 app_id；`None` 表示不持久化。
    pub app_id: Option<String>,
}

/// 启动软渲染应用。返回错误时，窗口一定没能跑起来（与 `eframe::run_native` 语义一致）。
pub fn run_soft(options: SoftOptions, app_creator: AppCreator) -> Result<(), String> {
    let event_loop: EventLoop<UserEvent> = EventLoop::with_user_event()
        .build()
        .map_err(|e| format!("创建事件循环失败：{e}"))?;

    // softbuffer 上下文必须在事件循环外创建（它要 owned display handle）。
    let softbuffer_ctx = softbuffer::Context::new(event_loop.owned_display_handle())
        .map_err(|e| format!("创建软渲染表面上下文失败：{e}"))?;

    let egui_ctx = Context::default();

    // egui 请求重绘 → 通过事件队列唤醒窗口重绘。MVP 忽略请求里的延迟，立即重绘，
    // 正确但偏积极；后续可读 `ViewportOutput::repaint_delay` 做节流。
    let proxy = event_loop.create_proxy();
    egui_ctx.set_request_repaint_callback(move |_info| {
        let _ = proxy.send_event(UserEvent::RequestRepaint);
    });

    let storage_arc = options
        .app_id
        .as_deref()
        .and_then(FileStorage::from_app_id)
        .map(|s| Arc::new(std::sync::Mutex::new(s)));

    let mut app = SoftApp {
        viewport: options.viewport,
        app_creator: Some(app_creator),
        egui_ctx: Some(egui_ctx),
        softbuffer_ctx: Some(softbuffer_ctx),
        storage_arc,
        window: None,
        surface: None,
        egui_winit: None,
        renderer: Renderer::new(),
        app: None,
    };

    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("事件循环异常退出：{e}"))
}

struct SoftApp {
    viewport: egui::ViewportBuilder,
    app_creator: Option<AppCreator>,
    egui_ctx: Option<Context>,
    softbuffer_ctx: Option<softbuffer::Context<OwnedDisplayHandle>>,
    storage_arc: Option<Arc<std::sync::Mutex<FileStorage>>>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<OwnedDisplayHandle, Arc<Window>>>,
    egui_winit: Option<egui_winit::State>,
    renderer: Renderer,
    app: Option<Box<dyn eframe::App>>,
}

impl ApplicationHandler<UserEvent> for SoftApp {
    fn resumed(&mut self, elwt: &ActiveEventLoop) {
        // 建窗：复用 egui_winit 对 ViewportBuilder 的完整解析（尺寸 / 标题 / 图标 / 边框）。
        let ctx = self.egui_ctx.as_ref().expect("事件循环启动前已建好 egui 上下文");
        let window = match egui_winit::create_window(ctx, elwt, &self.viewport) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("软渲染建窗失败：{e}");
                elwt.exit();
                return;
            }
        };

        // 建 softbuffer 表面（窗口的 CPU 位图）。
        let surface = match self
            .softbuffer_ctx
            .as_ref()
            .and_then(|c| softbuffer::Surface::new(c, window.clone()).ok())
        {
            Some(s) => s,
            None => {
                log::error!("软渲染表面创建失败");
                elwt.exit();
                return;
            }
        };

        // egui 输入状态机。
        let egui_winit = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        // 构造应用：storage 与后端共享同一份（关闭 / 重启时后端要能落盘）。
        let storage: Option<Box<dyn eframe::Storage>> = self
            .storage_arc
            .as_ref()
            .map(|a| Box::new(SharedStorage(a.clone())) as Box<dyn eframe::Storage>);

        let app = match self.app_creator.take().expect("只创建一次")(
            &self.egui_ctx.as_ref().unwrap(),
            storage,
        ) {
            Ok(app) => app,
            Err(e) => {
                log::error!("软渲染应用初始化失败：{e}");
                elwt.exit();
                return;
            }
        };

        self.window = Some(window);
        self.surface = Some(surface);
        self.egui_winit = Some(egui_winit);
        self.app = Some(app);

        // 首帧：egui 会请求重绘，但稳妥起见主动排一帧。
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, elwt: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window_id != window.id() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                self.finish(elwt);
            }
            WindowEvent::RedrawRequested => match self.redraw() {
                Ok(true) => self.finish(elwt),
                Ok(false) => {}
                Err(e) => {
                    log::error!("软渲染一帧失败：{e}");
                    self.finish(elwt);
                }
            },
            _ => {
                let repaint = self
                    .egui_winit
                    .as_mut()
                    .map(|s| s.on_window_event(&window, &event).repaint)
                    .unwrap_or(false);
                if repaint {
                    window.request_redraw();
                }
            }
        }
    }

    fn user_event(&mut self, _elwt: &ActiveEventLoop, _event: UserEvent) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
        elwt.set_control_flow(ControlFlow::Wait);
    }
}

impl SoftApp {
    /// 画一帧，返回 `true` 表示应用请求关闭（egui 发了 `ViewportCommand::Close`）。
    fn redraw(&mut self) -> Result<bool, String> {
        let window = self.window.clone().expect("窗口已建");
        let size = window.inner_size();
        let width_px = size.width.max(1);
        let height_px = size.height.max(1);
        let width = NonZeroU32::new(width_px).expect("非零");
        let height = NonZeroU32::new(height_px).expect("非零");

        let surface = self.surface.as_mut().expect("表面已建");
        surface
            .resize(width, height)
            .map_err(|e| format!("调整软渲染表面失败：{e}"))?;

        let raw_input = self
            .egui_winit
            .as_mut()
            .expect("egui 状态已建")
            .take_egui_input(&window);

        let ctx = self.egui_ctx.clone().expect("egui 上下文已建");
        // 清屏色沿用应用声明（否则 resize 瞬间、或 egui 没画到的边缘会露黑）。
        let theme = ctx.system_theme().unwrap_or(egui::Theme::Dark);
        let style = ctx.style_of(theme);
        let clear_color = self
            .app
            .as_ref()
            .map(|app| app.clear_color(&style.visuals))
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let clear_u32 = ((clear_color[0] * 255.0).round() as u32) << 16
            | ((clear_color[1] * 255.0).round() as u32) << 8
            | (clear_color[2] * 255.0).round() as u32;

        let mut frame = eframe::Frame::_new_kittest();
        let full_output = {
            let app = self.app.as_mut().expect("应用已建");
            ctx.run_ui(raw_input, |ui| {
                app.ui(ui, &mut frame);
            })
        };

        // 处理视口命令：关闭、拖拽、缩放、最大/最小化、聚焦、标题。
        let mut close_requested = false;
        if let Some(vo) = full_output.viewport_output.get(&egui::ViewportId::ROOT) {
            for cmd in &vo.commands {
                match cmd {
                    egui::ViewportCommand::Close => close_requested = true,
                    egui::ViewportCommand::StartDrag => {
                        let _ = window.drag_window();
                    }
                    egui::ViewportCommand::Title(t) => window.set_title(t),
                    egui::ViewportCommand::Focus => window.focus_window(),
                    egui::ViewportCommand::Maximized(m) => window.set_maximized(*m),
                    egui::ViewportCommand::Minimized(m) => window.set_minimized(*m),
                    egui::ViewportCommand::BeginResize(dir) => {
                        if let Some(d) = to_winit_resize(dir) {
                            let _ = window.drag_resize_window(d);
                        }
                    }
                    _ => {}
                }
            }
        }

        // 剪贴板 / 光标等非渲染输出。
        if let Some(state) = self.egui_winit.as_mut() {
            state.handle_platform_output(&window, full_output.platform_output);
        }

        // tessellate 成三角形 → CPU 光栅化 → 贴窗。
        let primitives = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        {
            let mut buffer = surface
                .buffer_mut()
                .map_err(|e| format!("获取软渲染缓冲失败：{e}"))?;
            buffer.fill(clear_u32);
            let fb = unsafe {
                std::slice::from_raw_parts_mut(
                    buffer.as_mut_ptr() as *mut [u8; 4],
                    width_px as usize * height_px as usize,
                )
            };
            self.renderer.render(
                fb,
                width_px as usize,
                height_px as usize,
                &primitives,
                &full_output.textures_delta,
                full_output.pixels_per_point,
            );
            buffer
                .present()
                .map_err(|e| format!("呈现软渲染帧失败：{e}"))?;
        }

        Ok(close_requested)
    }

    /// 关窗：先落盘（应用状态 + 草稿），再退出事件循环。
    fn finish(&mut self, elwt: &ActiveEventLoop) {
        self.save_and_exit();
        elwt.exit();
    }

    fn save_and_exit(&mut self) {
        if let (Some(app), Some(arc)) = (self.app.as_mut(), self.storage_arc.as_ref()) {
            if let Ok(mut storage) = arc.lock() {
                app.save(&mut *storage);
                storage.flush();
            }
        }
    }
}

/// egui 的缩放方向 → winit 的缩放方向（两侧枚举定义一致，只是命名空间不同）。
fn to_winit_resize(dir: &egui::viewport::ResizeDirection) -> Option<winit::window::ResizeDirection> {
    use egui::viewport::ResizeDirection as E;
    use winit::window::ResizeDirection as W;
    Some(match dir {
        E::North => W::North,
        E::South => W::South,
        E::East => W::East,
        E::West => W::West,
        E::NorthEast => W::NorthEast,
        E::SouthEast => W::SouthEast,
        E::NorthWest => W::NorthWest,
        E::SouthWest => W::SouthWest,
    })
}
