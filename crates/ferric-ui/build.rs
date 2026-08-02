//! 构建号 = git 提交数（`git rev-list --count HEAD`），以 FERRIC_BUILD_NUMBER
//! 环境变量注入编译期。随每次 commit 自增、所有机器/clone 一致、git pull 不产生
//! 冲突——无需本地计数文件。仅供更新器比较新旧，不体现在版本号里。
//! 非 git 环境（如发行版源码包）取不到时回退为 0。
//!
//! 版本号 FERRIC_VERSION = 最近一次**发布 tag**（vX.Y.Z，去掉 v）。版本只在发版
//! 打 tag 时前进，平时提交不动它——发版是显式动作，不跟提交数挂钩。
//! 「末位每发一版 +1、满千进位第二位」的规则作用在发布序号上，落在生成下一个
//! tag 的命令里（见 .github/workflows/release.yml 头部注释），此外无需任何算术。
//! 旧式两段 tag（v0.1 / v0.2）不匹配三段 glob，自动忽略。
//! 取不到 tag（无 git / 浅克隆 / 尚未发过版）回退 Cargo.toml 的占位版本。

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

    // 版本号 = 最近的三段式发布 tag；发版 CI 打包前会把同一值写进 Cargo.toml
    // （release.yml「按标签定版」步），安装包元数据与这里显示的一致。
    let ver = Command::new("git")
        .args([
            "describe",
            "--tags",
            "--abbrev=0",
            "--match",
            "v[0-9]*.[0-9]*.[0-9]*",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().trim_start_matches('v').to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));
    println!("cargo:rustc-env=FERRIC_VERSION={ver}");

    // 打 tag / fetch 到新 tag 时重跑，刷新版本号（松散 tag 在 refs/tags/ 下，
    // fetch 来的多半进 packed-refs；两处都盯）。
    for p in ["refs/tags", "packed-refs"] {
        if let Ok(out) = Command::new("git")
            .args(["rev-parse", "--git-path", p])
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
    }

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

    bake_update_pins();
}

/// 把「更新服务器身份」烘进二进制。
///
/// 这是「怎么保证调用的是我指定的服务」的地基：客户端**永远不会**去
/// `/api/v1/crypto/pubkey` 取公钥（那样等于让对方自报家门），只用这里烘进来的值。
/// 只有真服务端的私钥能解开客户端生成的会话密钥，因而只有它能产出通过 SM4-GCM
/// 校验的响应 —— 假服务器只能造成拒绝服务，无法冒充。
///
/// 三个值都为空时，更新功能整体禁用（UI 会明说「本构建未配置更新服务器」），
/// **绝不回落到「去服务端问公钥」**。
///
/// `FERRIC_RELEASE_PUBKEY` 同时是**插件市场**的验签公钥：没烘入它的构建既装不了更新，
/// 也装不了插件（`market::install` 会直接拒绝）。这是有意的 —— 无法验证来源时，
/// 唯一安全的行为是不装，而不是"先装上再说"。
///
/// 发布构建示例：
/// ```text
/// FERRIC_SERVER_URL=http://updates.example.com/api/v1 \
/// FERRIC_SERVER_PUBKEY=04ab… FERRIC_RELEASE_PUBKEY=04cd… cargo build --release
/// ```
fn bake_update_pins() {
    for key in [
        "FERRIC_SERVER_URL",
        "FERRIC_SERVER_PUBKEY",
        "FERRIC_RELEASE_PUBKEY",
    ] {
        // 没有这一行，改了环境变量重新编译不会重跑 build.rs，
        // 二进制里会静默保留上一次的值——属于「发布了才发现」的坑。
        println!("cargo:rerun-if-env-changed={key}");
        let v = std::env::var(key).unwrap_or_default();
        println!("cargo:rustc-env={key}={}", v.trim());
    }
}
