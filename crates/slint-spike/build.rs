fn main() {
    // 两个独立的 spike：外壳基线（app）与大文本压力测试（rope）。
    // include_modules! 会把 OUT_DIR 下生成的全部模块一并纳入，两个 bin 共用。
    //
    // 不用 EmbedForSoftwareRenderer：它会编译期预光栅化字形，而 Lucide 图标字体
    // 会让那条路 panic（embed_glyphs.rs 的 large glyph y coordinate）。
    for f in ["ui/app.slint", "ui/rope.slint"] {
        slint_build::compile_with_config(
            f,
            slint_build::CompilerConfiguration::new().with_style("fluent".into()),
        )
        .unwrap_or_else(|e| panic!("Slint 编译失败（{f}）：{e}"));
    }
}