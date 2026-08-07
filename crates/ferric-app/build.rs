//! 仅对 Windows 目标：把 icons/icon.ico 与应用清单嵌进 exe 资源。
//!
//! cargo-packager 的 `icons` 只管安装包（NSIS 界面、开始菜单快捷方式等），
//! **exe 文件本身**在资源管理器/任务栏里的图标必须编译期嵌入 PE 资源，二者缺一不可。
//!
//! # 为什么必须嵌 DPI 清单
//!
//! 显示缩放 125%/150%（虚拟机与笔记本的常态）下，Windows 对**未声明 DPI 感知**
//! 的进程走 DWM 位图拉伸 —— 整个窗口按比例放大，出来就是「整体一片糊」。
//! winit 虽会在运行期调用 `SetProcessDpiAwarenessContext`，但那是尽力而为：
//! 远程桌面、注入的第三方 DLL 提前初始化 user32 等情况都可能让它失效且**静默**。
//! 清单在进程创建那一刻就生效，是唯一无条件可靠的声明方式。
//! `PerMonitorV2` 之外再留 `dpiAware=true` 一行，给 Win8.1 之前的系统当回退。

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    // 判断的是编译目标而不是宿主：从 Linux/macOS 交叉编译 Windows 产物时同样要嵌。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icons/icon.ico");
        res.set_manifest(MANIFEST);
        if let Err(e) = res.compile() {
            // 交叉编译宿主可能没有 rc 工具链（llvm-rc / windres）。图标/清单缺失只是
            // 外观与缩放问题，不值得为它打断构建；CI 的 Windows 跑道自带 rc.exe。
            println!("cargo:warning=嵌入 Windows 资源失败（缺 rc 工具链？）：{e}");
        }
    }
}
