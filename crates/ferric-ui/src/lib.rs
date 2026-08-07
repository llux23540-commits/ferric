//! ferric-ui —— egui 视图与应用外壳。
//!
//! 对外暴露 [`FerricApp`]（实现 `eframe::App`）、[`APP_NAME`] 与 [`launch`]
//! （启动期的渲染后端选择与自愈），供 ferric-app 启动。

mod app;
mod chrome;
mod fonts;
mod icons;
pub mod launch;
mod market;
mod mock;
mod net;
mod plugin_host;
mod release;
mod source;
mod theme;
mod tool;
mod updater;
mod views;
mod widgets;

pub use app::{FerricApp, APP_NAME};
pub use fonts::install_fonts;
pub use theme::Theme;
