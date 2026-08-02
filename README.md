# Ferric

跨平台原生 **Rust** 开发者工具箱。基于 [egui](https://github.com/emilk/egui) / eframe 的高性能即时模式 GUI（非 Tauri/Web 方案），单二进制，运行于 **Windows / macOS / Linux**。

![Ferric 截图](docs/screenshot.png)

## 下载

安装包见 [Releases](https://github.com/llux23540-commits/ferric/releases/latest)，按平台选：

| 你的机器 | 下载哪个 |
|---|---|
| Windows · Intel / AMD（x64，最常见） | `…windows-x86_64-setup.exe`（安装版）或 `…-portable.exe`（免安装单文件） |
| Windows · ARM（骁龙 Snapdragon 本） | `…windows-aarch64-setup.exe` 或对应 portable |
| Mac · Apple 芯片（M1–M4） | `…macos-aarch64.dmg` |
| Mac · Intel | `…macos-x86_64.dmg` |
| Linux · Intel / AMD（x64） | `…linux-x86_64.deb`（Debian/Ubuntu）或 `.AppImage`（任意发行版） |
| Linux · ARM64（树莓派等） | `…linux-aarch64.deb` 或对应 `.AppImage` |

> x64 = x86_64 = amd64 是同一个东西的三种叫法，Intel 和 AMD 的处理器都用它；
> aarch64 = ARM64（Apple 芯片、骁龙、树莓派同属这一架构，但系统各自要装各自的包）。

## 已实现（10 工具）

外壳：自绘无边框窗口（拖拽 / 最小化 / 最大化 / 关闭）、亮/暗主题、可拖拽调宽侧边栏、`Ctrl+K` 命令面板、工具收藏、全工具草稿持久化、CJK 字体自动加载。

工具：

| 工具 | 说明 |
|---|---|
| JSON 工具 | 格式化 / 压缩 / 校验 / 转义 / 去转义 / 键名排序，缩进 2·4·Tab，撤销重做，铺满式行号编辑区 + 折叠树视图，长行自动换行（可关，关后横向滚动） |
| 文本 / 文件对比 | 逐行 diff，差异直接高亮在左右编辑面板内（删除标左、新增标右，字符级标记），载入 / 拖入文件 |
| 时间戳 | Unix ↔ 日期时间，秒/毫秒，全量时区可搜索，自动识别多种日期格式 |
| JSON → YAML | JSON 转 YAML，实时校验 |
| SQL 格式化 | 格式化 / 压缩为单行，关键字大写开关 |
| UUID 生成器 | UUID v4 / v7 / v6 / v5（命名空间），大小写 / 无连字符，Raw / JSON，执行历史 |
| RSA 密钥对 | 256–4096 位，后台线程生成，PEM 输出 |
| 加密 / 解密文本 | AES / TripleDES / Rabbit（RFC 4503）/ RC4，OpenSSL 盐格式，与 crypto-js 兼容 |
| 国密 SM | SM4 对称、SM2 公钥加解密、SM3 摘要，一键生成 SM2 密钥对 |
| 正则表达式 | g/i/m/s/x 标志，分组捕获展示，常用语法备忘单 |

## 自动更新与插件市场

客户端可对接 [ferric-server](https://github.com/llux23540/ferric-server)：检查/下载安装包、
浏览并安装 WASM 插件。**没有 TLS，安全性靠三把独立的锁**，全部在编译期烘进二进制：

| 编译期变量 | 作用 |
|---|---|
| `FERRIC_SERVER_URL` | 服务端地址（`…/api/v1`） |
| `FERRIC_SERVER_PUBKEY` | 传输加密公钥（SM2）。客户端**永不**去 `/crypto/pubkey` 取——那等于让对方自报家门 |
| `FERRIC_RELEASE_PUBKEY` | 发布验签公钥。私钥永不上服务器，**安装包与插件都必须验签** |

```sh
FERRIC_SERVER_URL=http://updates.example.com/api/v1 \
FERRIC_SERVER_PUBKEY=04… FERRIC_RELEASE_PUBKEY=04… \
  cargo build --release -p ferric-app
```

三个值缺省时相关功能整体禁用，**绝不回落到「去服务端问公钥」**；未烘入验签公钥的构建
既装不了更新也装不了插件——无法验证来源时，唯一安全的行为是不装。

插件跑在 wasmtime 沙箱里，但沙箱管的是「能碰到什么」，管不了「算出什么」（一个伪造的
「加密工具」插件完全可以在沙箱内输出可预测的密文），所以插件与安装包走同一条离线签名链。
签名清单绑定了 slug，**换个身份重放也不行**。

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

### 打包发行版

打包配置在 `crates/ferric-app/Cargo.toml` 的 `[package.metadata.packager]`（cargo-packager）。

```sh
cargo install cargo-packager --locked
cargo build --release -p ferric-app
cargo packager --release --formats nsis   # Windows 安装包；macOS 用 dmg，Linux 用 deb / appimage
```

产物输出到 `target/release/`，如 `ferric_<版本>_x64-setup.exe`；
免安装便携版直接分发 `target/release/ferric.exe` 即可（单二进制，无外部依赖）。

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
