//! Ferric 桌面客户端入口。
//!
//! 迁 Slint 之后这里只剩一件事：**把窗口开出来**。
//!
//! egui 时代这个文件是一套「按计划逐个试渲染后端 + 记住哪个成功 + 崩了自动降级」
//! 的状态机（`launch::plan` / `begin` / `run_once` 循环 + wgpu/glow/软渲染三条
//! 分支），因为在无 GPU 驱动的机器上「哪个后端能用」只有到了那台机器才知道。
//!
//! Slint 的 software renderer 让那个问题整体消失 —— 它不碰任何 GPU API，
//! 任何机器上都一样能起来。于是入口回归成一行 `ferric_ui::run()`，
//! 失败时把原因写进 `startup.log` 并弹窗（发行版没有控制台，
//! 不弹窗用户看到的就是「双击了没反应」）。

// 发行版隐藏 Windows 控制台窗口。
// 代价是 stderr 没有任何去处 —— 启动失败必须落到日志文件 + 弹窗。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(e) = ferric_ui::run() {
        let detail = e.to_string();
        ferric_ui::launch::mark_failed(&detail);
        ferric_ui::launch::fatal_dialog(&detail);
        std::process::exit(1);
    }
}
