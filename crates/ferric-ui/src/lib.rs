//! ferric-ui —— egui 视图与应用外壳。
//!
//! 对外暴露 [`FerricApp`]（实现 `eframe::App`）、[`APP_NAME`] 与 [`launch`]
//! （启动期的渲染后端选择与自愈），供 ferric-app 启动。

mod app;
mod chrome;
mod fonts;
mod github;
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

/// 测试里跑一帧 UI 的包装：egui 0.36 起，`FullOutput::textures_delta` 若带着未应用的
/// 字体纹理就 drop，会触发 `debug_assert`（见 epaint `TexturesDelta::drop`）。
/// 单测不接 GPU，消费不掉这些 delta，统一在这里 clear 掉再返回。
#[cfg(test)]
pub(crate) trait RunUiExt {
    fn run_ui_cleared(
        &self,
        input: egui::RawInput,
        add_contents: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput;
}

#[cfg(test)]
impl RunUiExt for egui::Context {
    fn run_ui_cleared(
        &self,
        input: egui::RawInput,
        add_contents: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut out = self.run_ui(input, add_contents);
        out.textures_delta.clear();
        out
    }
}
