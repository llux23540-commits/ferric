# Ferric

跨平台原生 **Rust** 开发者工具箱。基于 [egui](https://github.com/emilk/egui) / eframe 的高性能即时模式 GUI（非 Tauri/Web 方案），单二进制，运行于 **Windows / macOS / Linux**。

## 已实现（v0.4.2 · MVP）

外壳：自绘无边框窗口（拖拽 / 最小化 / 最大化 / 关闭）、亮/暗主题、可拖拽调宽侧边栏、`Ctrl+K` 命令面板、工具收藏、配置持久化、CJK 字体自动加载。

工具：

| 工具 | 说明 |
|---|---|
| JSON 工具 | 格式化 / 压缩 / 校验 / 转义 / 去转义，缩进 2·4·Tab，排序键 |
| 文本 / 文件对比 | 逐行 diff，支持拖入文件到左 / 右侧 |
| 时间戳 | Unix ↔ 日期时间，秒/毫秒，多时区，逐项 & `YYYYMMDDHHMMSS` 快速输入 |
| UUID 生成器 | UUID v4/v7 · ULID · NanoID，大小写 / 无连字符，Raw / JSON |

后续：JSON→YAML、SQL 格式化、RSA 密钥对、对称加解密、正则表达式、国密 SM。

## 结构

```
crates/
  ferric-core/   纯逻辑（无 GUI），带单元测试
  ferric-ui/     egui 视图与外壳；新增工具 = 加 views/*.rs + registry() 注册一行
  ferric-app/    eframe 入口
```

## 开发

需要 Rust stable。

```sh
cargo run -p ferric-app     # 运行
cargo test                  # 核心逻辑单测
cargo clippy --all-targets  # 静态检查
```

### Windows on ARM64 说明

本仓库默认针对 `aarch64-pc-windows-msvc`（原生）。原生构建需安装
`Microsoft.VisualStudio.Component.VC.Tools.ARM64`（VS Build Tools 里的 “MSVC ARM64 build tools”）。
用 `setup.exe modify --quiet` 静默安装时**必须以管理员身份运行**（否则报 5007）。

x64 模拟工具链虽能编译，但模拟进程无法访问 GPU，GUI 跑不起来——请用原生 aarch64 构建。

#### 渲染后端（DX12）

GUI 用 wgpu 后端，Windows 走 **DX12**。注意 eframe 0.29 默认没给 wgpu 开 `dx12`
feature，因此 `ferric-app` 在自己的依赖里**显式启用** `wgpu = { features = ["dx12"] }`
（feature 会合并进 eframe 共用的 wgpu 构建）。否则在没有 Vulkan 驱动、OpenGL 仅 1.1 的
环境（如 **QEMU 虚拟机**：只有 “Microsoft Basic Render Driver” 软件 **WARP** 适配器）会
报 `NoSuitableAdapterFound`。

WARP 软件光栅化器只支持 **不透明** 表面，所以窗口默认 `with_transparent(false)`；
有硬件 GPU 时可在 `crates/ferric-app/src/main.rs` 改回 `true` 获得圆角透明效果。

## 许可

MIT
