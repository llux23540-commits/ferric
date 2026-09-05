# Ferric

跨平台原生 **Rust** 开发者工具箱。基于 [Slint](https://slint.dev) 的声明式 GUI + **纯 CPU 软件渲染**（非 Tauri/Web 方案），单二进制，运行于 **Windows / macOS / Linux**。

不碰任何 GPU API：任何机器（含无显卡驱动的虚拟机、精简版 Windows、远程桌面）都能打开，
进程内存约 **35MB**。

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
>
> **Windows 升级**：直接双击新版 setup 即可**覆盖安装**，无须先卸载旧版
>（安装器检测到旧版会静默覆盖，不再弹「是否先卸载」）；应用内更新更进一步 ——
> 点「安装」后走静默覆盖并自动重启 Ferric，全程无须点任何安装向导。
> 安装按当前用户进行，不需要管理员权限（无 UAC 弹窗）。
> 首次运行时 Windows SmartScreen 的「已保护你的电脑」提示来自**未购买代码签名证书**，
> 点「更多信息 → 仍要运行」即可；这与安装方式无关，签名证书就绪前无法消除。
>
> **Mac 首次打开**：应用是 ad-hoc 签名（未做 Apple 公证），首次会提示无法验证——
> 系统设置 → 隐私与安全性 → 底部「仍要打开」。若提示「已损坏」：`xattr -cr /Applications/Ferric.app`。

## 已实现（10 工具）

外壳：自绘无边框窗口（拖拽 / 最小化 / 最大化 / 关闭）、亮/暗主题、可拖拽调宽侧边栏、`Ctrl+K` 命令面板、工具收藏、全工具草稿持久化、CJK 字体自动加载。

工具：

| 工具 | 说明 |
|---|---|
| JSON 工具 | 格式化 / 压缩 / 校验 / 转义 / 去转义（多层转义与内嵌 JSON 字符串一次剥完）/ 键名排序，`Ctrl+F` 搜索（Enter/F3 逐个跳转），三连击直接选中整个字符串值，缩进 2·4·Tab，撤销重做，铺满式行号编辑区 + 折叠树视图，长行自动换行（可关，关后横向滚动） |
| 文本 / 文件对比 | 逐行 diff，差异直接高亮在左右编辑面板内（删除标左、新增标右，字符级标记），左右同步滚动，`Ctrl+F` 搜索（聚焦哪侧搜哪侧，未聚焦两侧一起），载入 / 拖入文件 |
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

### 更新是怎么走完的

「检查 → 下载 → 安装」里只有**最后一步**需要人点：

1. 启动 4 秒后自动检查（跨启动节流，最短间隔 6 小时；设置里可关）；
2. 发现新版**自动在后台下载**，并做 sha256 + 魔数 + 离线签名三重校验；
3. 校验通过后弹出更新框，点「立即安装」即覆盖安装并退出；也可先「稍后」，顶栏的「安装 vX」按钮仍在。

**后台绝不自动安装**：那一步会关掉用户正在用的应用，必须由他自己决定。
自动后台下载也只对**内置服务器**开放；自定义更新源只提示新版本，不下载不执行。

### 没有服务器也能跑：演示数据

设置 → 数据源 → 自动 / 服务器 / **演示**。没烘入 `FERRIC_SERVER_URL` 的构建默认走演示：
插件市场有一份固定的插件目录，更新那边有一个「新版本」，下载进度真的会走
（分块 + 真实耗时），装完的状态会存盘。

演示分支**碰不到任何安全边界**：不写插件目录（那条路只接受验签通过的字节）、
不执行任何安装程序、界面上一律标注「演示数据」。它能造成的最坏结果就是
「界面上多了几条假数据」。

### 插件装完立刻生效

装 / 卸插件之后不必重启：外壳会在当前帧渲染结束后重新加载插件目录，
保留当前选中的工具与各插件的输入草稿。市场页支持「全部更新」（逐个装，带进度条），
进页面即自动拉取列表。

插件跑在 wasmtime 沙箱里，但沙箱管的是「能碰到什么」，管不了「算出什么」（一个伪造的
「加密工具」插件完全可以在沙箱内输出可预测的密文），所以插件与安装包走同一条离线签名链。
签名清单绑定了 slug，**换个身份重放也不行**。

## 结构

```
crates/
  ferric-core/   纯逻辑（无 GUI），带单元测试
  ferric-ui/     Slint 视图与外壳
    ui/*.slint     视图：app（外壳 + 各工具）/ widgets（复用组件）/
                   theme（设计令牌）/ editor（视口虚拟化编辑区）
    src/views/*.rs 各工具的状态与业务
    src/editor.rs  rope 文本缓冲（光标 / 选区 / 撤销），只渲染可见行
  ferric-app/    入口（建窗 → 跑事件循环）
```

新增一个工具：写 `views/<id>.rs` + 在 `ui/app.slint` 加视图组件与 `current.id`
分支 + 在 `views::registry()` 注册一行 + 在 `state.rs` 的 `Shell::with_buffer`
加编辑区映射。漏了 `.slint` 那步，内容区就是一片空白 —— 侧栏照旧有条目。

### 为什么编辑区是自己写的

Slint 的原生 `TextEdit` 对**整篇文档**布局，而软件渲染器的坐标空间是 i16
（上限 32767 像素）。实测**超过约 2190 行直接 panic**，且内存放大 107×。

所以 `src/editor.rs` + `ui/editor.slint` 自己做视口虚拟化：文本存 rope，
只把可见的那几十行交给渲染层，布局高度恒等于视口高度、与文档多大无关。
粘进几十万行 JSON 也不会崩、不会卡。

## 开发

需要 Rust stable。

```sh
cargo run -p ferric-app     # 运行
cargo test                  # 核心逻辑单测
cargo clippy --all-targets  # 静态检查
```

`[profile.dev]` 里把 `debug` 压到 `line-tables-only`：`ui/*.slint` 被编译成
**单个 9.8MB / 97k 行**的 Rust 文件，完整 DWARF 下 rustc 的 LLVM 线程会
`out of memory`（12GB 机器上默认 `cargo test` 因此构建不出来）。backtrace 的
文件与行号仍在；确实要看局部变量时临时 `CARGO_PROFILE_DEV_DEBUG=2 cargo build -j 1`。

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

#### 渲染后端

GUI 走 Slint 的 **software renderer**：纯 CPU 光栅化，不创建任何 GPU 上下文，
因此没有「挑不到适配器」这回事 —— 无论有没有显卡驱动，行为一致。

egui/wgpu 时代那套「按顺序试 DX12 / Vulkan / OpenGL + 跨启动自愈 + 记住哪个
成功」的机制随之删除（连同 `WGPU_BACKEND` 环境变量）。同一台机器上的内存对比：

| 渲染路径 | 进程内存 |
|---|---|
| egui + wgpu（退化成 WARP 软件光栅化） | ~611 MB |
| **Slint software renderer** | **~35 MB** |


## 排障

### 整个界面发糊（尤其虚拟机）

按嫌疑从大到小排查：

1. **Windows 显示缩放 125%/150% + 旧版本**：旧版 exe 未嵌 DPI 清单，声明一旦失效
   （远程桌面等场景）就会被系统整窗位图拉伸 —— 那是无解的糊。现版本已在 exe 里
   嵌入 `PerMonitorV2` 清单，进程创建即生效，请升级后再看。
2. **虚拟机软件把客户机画面拉伸显示**（VMware「自动适应客户机」、VirtualBox 缩放
   模式等）：宿主侧的位图缩放，任何应用都救不了。把客户机分辨率设为与显示窗口
   1:1，或关闭 hypervisor 的缩放。
3. **界面缩放非 100%**：设置 → 界面缩放 调回 100% 对比。缩放是整窗 scale
   factor（不是位图放大），但非整数倍下字形栅格化本身会略软。

渲染路径不用查：只有软件渲染一条（见上文「渲染后端」），锐度与显卡驱动无关。

### 打不开

启动失败会弹窗说明，并把详情写进 `startup.log`（发行版隐藏了控制台，
stderr 没有去处，日志文件是唯一线索）。

不再有「换个渲染后端试试」这一步 —— 软件渲染不依赖显卡驱动，
打不开的原因只会是缺系统库或显示服务器连不上，两者都会写进日志。

### 状态与日志的位置

| 平台 | 目录 |
|---|---|
| Windows | `%APPDATA%\ferric\data\` |
| macOS | `~/Library/Application Support/ferric/` |
| Linux | `~/.local/share/ferric/` |

里面有界面状态 `app.ron`、`launch.json`（启动诊断标记）与 `startup.log`。
删掉即恢复出厂设置 —— 从 egui 版升级上来的用户，那份 `app.ron` 会被直接读取
（同目录、同文件名、同结构），设置与各工具草稿都不会丢。

### 界面中文显示成方块

系统里没有中文字体。Ferric 会依次找微软雅黑 / 黑体 / 宋体（含 `%WINDIR%` 与用户字体
目录）、macOS 苹方、Linux 的 Noto CJK / 文泉驿；都找不到时会提示。装一个中文字体
（如 Noto Sans SC）即可。

### Windows：拖动窗口发花 / Alt+Tab 切回闪一下旧画面

这些是 **DX12 flip-model 呈现与窗口合成不同步**的老毛病，而 Ferric 已经
不走 GPU 了 —— 软件渲染直接把位图交给系统合成，没有交换链、没有呈现队列，
因此那一整类问题（连同曾经的「Alt+Tab 卡顿缓解」开关与
`WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT` 环境变量）都不存在了。

仍然看到拖影，请提 issue 并附上 `startup.log`。

### CPU 占用偏高

Slint 是 retained mode：**只有 property 变化时才重画那一小块脏区域**，
静置时进程基本不耗 CPU。egui 时代「每帧重建整棵 UI」带来的那些补偿逻辑
（时间戳挂表要对齐秒边界、失焦零调度、静置自动停表）因此全部删掉了。

时间戳工具的「实时刷新」默认**关**：一个一直在跳的数字容易让人以为界面卡了，
而多数用户是来做一次换算的。需要挂表时在工具里打开即可。


## 许可

MIT
