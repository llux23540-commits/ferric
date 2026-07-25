//! ferric-ui —— egui 视图与应用外壳。
//!
//! 对外暴露 [`FerricApp`]（实现 `eframe::App`）与 [`APP_NAME`]，供 ferric-app 启动。

mod app;
mod chrome;
mod fonts;
mod icons;
mod market;
mod net;
mod plugin_host;
mod release;
mod theme;
mod tool;
mod updater;
mod views;
mod widgets;

pub use app::{FerricApp, APP_NAME};
pub use fonts::install_fonts;
pub use theme::Theme;
