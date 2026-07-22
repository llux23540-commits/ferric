//! 构建号自增：每次源码变化触发重编译时 +1，写入 build_number.txt，
//! 并以 FERRIC_BUILD_NUMBER 环境变量注入编译期（UI 版本文案拼接用）。
//! 无改动的 cargo build 不会重跑本脚本，构建号保持不变。

use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let counter = Path::new(&dir).join("build_number.txt");
    let n: u64 = fs::read_to_string(&counter)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
        + 1;
    let _ = fs::write(&counter, n.to_string());
    println!("cargo:rustc-env=FERRIC_BUILD_NUMBER={n}");
}
