//! 构建号 = git 提交数（`git rev-list --count HEAD`），以 FERRIC_BUILD_NUMBER
//! 环境变量注入编译期（UI 版本文案拼接用）。随每次 commit 自增、所有机器/clone
//! 一致、git pull 不产生冲突——无需本地计数文件。
//! 非 git 环境（如发行版源码包）取不到时回退为 0。

use std::process::Command;

fn main() {
    let n = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0".to_owned());
    println!("cargo:rustc-env=FERRIC_BUILD_NUMBER={n}");

    // HEAD 移动（提交 / 切换分支）时重跑本脚本刷新构建号。
    // logs/HEAD 每次 commit/checkout/reset 都会追加，是可靠的触发点；
    // 用 rev-parse --git-path 拿到正确路径（兼容 worktree）。
    if let Ok(out) = Command::new("git")
        .args(["rev-parse", "--git-path", "logs/HEAD"])
        .output()
    {
        if out.status.success() {
            if let Ok(p) = String::from_utf8(out.stdout) {
                let p = p.trim();
                if !p.is_empty() {
                    println!("cargo:rerun-if-changed={p}");
                }
            }
        }
    }
    println!("cargo:rerun-if-changed=Cargo.toml");
}
