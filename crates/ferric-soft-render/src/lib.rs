//! 纯 CPU 软渲染后端：把 egui 的界面**不经过 GPU** 直接画进内存位图，再贴到窗口。
//!
//! 为什么需要它：egui / eframe 默认走 wgpu（或 glow），在无 GPU 的虚拟机、精简版
//! Windows、远程桌面上，这些 API 会退化成 WARP / llvmpipe 等软件光栅化，那一整层
//! native 抽象 + 虚拟 GPU 驱动能吃掉数百 MB 进程内存（Ferric 实测 5MB JSON 编辑器
//! 静默占用 ~650MB，其中 Rust 堆只有几十 MB）。本后端绕开所有 GPU API，进程内存
//! 降到与「一个 winit 窗口 + 一块位图」相当的水平。
//!
//! 界面完全不变：喂进去的是同一个 `eframe::App`（`App::ui`），变的只是最后把
//! tessellate 出来的三角形画到哪 —— 这里画进 CPU 位图，而不是交给 GPU。

mod backend;
mod raster;
mod storage;
mod texture;

pub use backend::{run_soft, AppCreator, SoftOptions};
