//! 仅对 Windows 目标：把 icons/icon.ico 嵌进 exe 资源。
//!
//! cargo-packager 的 `icons` 只管安装包（NSIS 界面、开始菜单快捷方式等），
//! **exe 文件本身**在资源管理器/任务栏里的图标必须编译期嵌入 PE 资源，二者缺一不可。

fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    // 判断的是编译目标而不是宿主：从 Linux/macOS 交叉编译 Windows 产物时同样要嵌。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icons/icon.ico");
        if let Err(e) = res.compile() {
            // 交叉编译宿主可能没有 rc 工具链（llvm-rc / windres）。图标缺失只是外观
            // 问题，不值得为它打断构建；CI 的 windows-latest 自带 rc.exe，不会走到这。
            println!("cargo:warning=嵌入 Windows 图标失败（缺 rc 工具链？）：{e}");
        }
    }
}
