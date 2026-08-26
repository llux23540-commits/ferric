//! 应用自动更新：检查 → 换票 → 下载 → 校验 → 验签 → 拉起安装。
//!
//! 全程在后台线程跑（照抄 `views/rsa.rs` 的「线程 + mpsc + 每帧 try_recv」模式），
//! 但**轮询点在 `app.rs` 的 `App::ui` 顶层**而不是某个视图里 —— 更新是全局的，
//! 挂在视图里会导致用户不切到那个页面就永远收不到结果。
//!
//! # 三道锁（见 `net` 与 `release` 模块头部）
//!
//! ① 传输公钥固定 → 对面必须是指定服务端；② sha256 取自**加密信道**（绝不用明文的
//! `X-Sha256` 响应头）→ 字节没在路上被换；③ 离线签名 → 服务器被拿下也推不了恶意包。
//!
//! # 不信任服务端的判断
//!
//! `/version/latest` 的参数走 URL query，**在信封之外**，中间人可以改写。把
//! `current_build` 改成一个巨大的值，服务端就会「正确地」算出 `has_update=false` 并
//! 「正确地」加密返回 —— 客户端全部校验通过却被永久压制在旧版本上。
//! 所以这里**本地重算** has_update / force，并逐字校验响应回显的 channel/platform/arch。

use crate::net::{self, NetErr, ServerProfile};
use crate::release;
use crate::source::Source;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

/// 本客户端的版本标识。版本 = 最近一次发布 tag（见 ferric-ui/build.rs），只在
/// 发版时前进；比较新旧一律用 build —— git 提交数单调递增，与版本格式解耦。
pub fn my_version() -> &'static str {
    env!("FERRIC_VERSION")
}
pub fn my_build() -> i64 {
    env!("FERRIC_BUILD_NUMBER").parse().unwrap_or(0)
}

/// 本机平台/架构，取值域与服务端 `PLATFORMS`/`ARCHES` 白名单一致。
pub(crate) fn platform_arch() -> Result<(&'static str, &'static str), String> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        other => return Err(format!("平台 {other} 不支持自动更新")),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => return Err(format!("架构 {other} 不支持自动更新")),
    };
    Ok((os, arch))
}

/// 服务端下发的版本信息（全部取自**加密信道**）。
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub id: i64,
    pub version: String,
    pub build: i64,
    pub notes: String,
    pub sha256: String,
    pub size: i64,
    pub min_supported_build: i64,
    pub signature: String,
    pub ext: String,
    /// 本地重算的结果，不采信服务端返回的同名字段
    pub force: bool,
}

#[derive(Debug, Clone, Default)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    /// 已是最新（**只有校验全部通过才会进这个状态**）
    UpToDate,
    Available(ReleaseInfo),
    Downloading {
        done: u64,
        total: u64,
    },
    /// 下载完并通过 sha256 + 魔数 + 签名三重校验，可以安装了
    Ready {
        info: ReleaseInfo,
        file: PathBuf,
    },
    /// 检查/下载失败。**必须与 UpToDate 区分显示** —— 否则中间人丢包就能伪装成「已最新」
    Failed(String),
}

enum Msg {
    Progress { done: u64, total: u64 },
    Checked(Box<Result<Option<ReleaseInfo>, String>>),
    Ready(Box<(ReleaseInfo, PathBuf)>),
    Failed(String),
}

/// 启动后多久做第一次自动检查（秒）。
///
/// 从 4 秒推到 25 秒：4 秒正好落在「用户刚打开、开始点第一下」的窗口里，
/// 而检查之后紧接着的是后台下载 —— 下载全程界面都在被反复重画（见
/// [`PROGRESS_BEAT`]）。软件渲染的机器上，这就是「一打开就很卡」的时间线。
/// 更新是完全不着急的事，让开这段最需要响应的时间。
const FIRST_CHECK_DELAY_SECS: f64 = 25.0;

/// 后台下载期间「因为进度变了」而重绘的最小间隔。
///
/// 下载期间界面上唯一会变的东西是顶栏一行「更新下载中 NN%」。egui 没有局部重绘：
/// 每请求一次重绘 = 整窗重新排版、重新三角化、重新光栅化。为了一个百分比数字
/// 把整窗按 10fps 重画，在软件光栅化（WARP / llvmpipe）的机器上足以让整机发卡。
///
/// 500ms（2fps）对一个百分比数字完全够看。此前是 100ms，再之前是「每收到一块
/// 数据就重绘」（≈33fps）—— 一路都是同一个错误的不同剂量。
pub(crate) const PROGRESS_BEAT: std::time::Duration = std::time::Duration::from_millis(500);

/// 后台任务在跑时的兜底轮询间隔。
///
/// 正常路径**不靠它**：每个发消息的地方都紧跟着一次 `request_repaint*`，
/// 结果一到就会被唤醒。它只是防「某次唤醒丢了就再也醒不过来」的保险，
/// 因此可以很慢 —— 1 秒醒一次不会让任何人看出延迟。
pub(crate) const IDLE_BEAT: std::time::Duration = std::time::Duration::from_secs(1);

/// 两次自动检查之间的最小间隔。检查更新会把本机版本号发给服务器，
/// 没必要频繁 —— 一天四次足够，用户随时可以手动点。
pub const AUTO_CHECK_INTERVAL_SECS: u64 = 6 * 3600;

#[derive(Default)]
pub struct Updater {
    pub phase: Phase,
    /// 上次**成功**检查的时刻。长期检查不成功要提示用户 —— 这是对抗
    /// 「中间人直接丢包压制更新」的唯一手段。
    pub last_ok: Option<std::time::SystemTime>,
    rx: Option<Receiver<Msg>>,
    /// 自动检查的时刻（egui 的 `input().time` 轴），首帧时排定。
    auto_check_at: Option<f64>,
    /// 本次运行是否已经自动下载过一次。
    ///
    /// 失败了也不再自动重试：更新包动辄几十 MB，自动重试很容易变成
    /// 「每帧都在重下」的流量灾难。手动按钮随时可以再来一次。
    auto_downloaded: bool,
    /// 「已就绪」是否已经通知过（否则每帧都会弹一次）。
    notified: bool,
}

/// 一次 `tick` 之后值得让外壳知道的事。
#[derive(Debug, PartialEq, Eq)]
pub enum Tick {
    Nothing,
    /// 后台下载完成且三重校验通过，可以安装了（外壳弹一次提示）。
    /// 版本号随事件一起带出去，省得外壳再去 `phase` 里翻一遍。
    ReadyToInstall {
        version: String,
    },
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Downloading { .. })
    }

    /// 后台流水线：到点自动检查 → 发现新版自动下载 → 就绪后通知外壳。
    ///
    /// 每帧调用（`App::ui` 顶层，紧跟 `poll` 之后）。**只调度，不安装** ——
    /// 拉起安装程序会关掉正在用的应用，那必须是用户点出来的动作，
    /// 绝不能由后台替他决定（见 `Source::allows_install`）。
    ///
    /// - `source`：当前数据源；`None`（没配服务器又关了演示）直接什么都不做。
    /// - `auto`：设置里的「自动检查并后台下载」开关。
    /// - `stale`：距上次成功检查是否已超过 [`AUTO_CHECK_INTERVAL_SECS`]，
    ///   由外壳依据持久化的时间戳判断（跨启动节流，否则每开一次应用就查一次）。
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        source: Option<&Source>,
        auto: bool,
        stale: bool,
    ) -> Tick {
        let Some(src) = source else {
            return Tick::Nothing;
        };
        if !auto {
            return Tick::Nothing;
        }

        // ① 到点自动检查（只在完全空闲、且这一轮还没查过的时候）
        if stale && self.rx.is_none() && matches!(self.phase, Phase::Idle) {
            let now = ctx.input(|i| i.time);
            let due = *self
                .auto_check_at
                .get_or_insert(now + FIRST_CHECK_DELAY_SECS);
            if now >= due {
                self.check(src.clone(), ctx);
            } else {
                // 空闲时应用是不出帧的，必须约好那一刻醒过来，否则这次检查永远不发生
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(due - now));
            }
        }

        // ② 发现新版 → 后台下载。只对可信来源自动下载：自定义更新源可能是
        //    用户被诱导改的，不能让它在后台悄悄往盘上拉东西。
        let to_download = match &self.phase {
            Phase::Available(info) if !self.auto_downloaded && src.allows_auto_download() => {
                Some(info.clone())
            }
            _ => None,
        };
        if let Some(info) = to_download {
            self.auto_downloaded = true;
            self.download(src.clone(), info, ctx);
        }

        // ③ 就绪 → 通知一次
        match &self.phase {
            Phase::Ready { info, .. } if !self.notified => {
                self.notified = true;
                Tick::ReadyToInstall {
                    version: info.version.clone(),
                }
            }
            _ => Tick::Nothing,
        }
    }

    /// 启动一次检查。UI 线程只负责起线程，SM2 标量乘法绝不能放主线程。
    pub fn check(&mut self, source: Source, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = check_blocking(&source);
            let _ = tx.send(Msg::Checked(Box::new(r)));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.phase = Phase::Checking;
    }

    /// 下载 + 校验 + 验签。只在内置服务器下允许调用（自定义服务器降级为仅通知）。
    pub fn download(&mut self, source: Source, info: ReleaseInfo, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let r = download_blocking(&source, &info, &mut |done, total| {
                let _ = tx2.send(Msg::Progress { done, total });
                // 节流：每块数据到达都立即 request_repaint 会把界面拖进 30+fps 的
                // 重绘风暴（软件渲染机器整机变卡，实测归因至此行）。
                // repaint_after 会把多次请求合并成一个醒点，间隔见 PROGRESS_BEAT。
                ctx2.request_repaint_after(PROGRESS_BEAT);
            });
            match r {
                Ok(path) => {
                    let _ = tx.send(Msg::Ready(Box::new((info, path))));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Failed(e));
                }
            }
            ctx2.request_repaint();
        });
        self.rx = Some(rx);
        self.phase = Phase::Downloading { done: 0, total: 0 };
    }

    /// 每帧调用（`App::ui` 顶层）。用 `request_repaint_after` 而非死循环重绘。
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress { done, total }) => {
                    self.phase = Phase::Downloading { done, total };
                }
                Ok(Msg::Checked(r)) => {
                    self.phase = match *r {
                        Ok(Some(info)) => {
                            self.last_ok = Some(std::time::SystemTime::now());
                            Phase::Available(info)
                        }
                        Ok(None) => {
                            self.last_ok = Some(std::time::SystemTime::now());
                            Phase::UpToDate
                        }
                        // 失败绝不映射成「已是最新」
                        Err(e) => Phase::Failed(e),
                    };
                    self.rx = None;
                    return;
                }
                Ok(Msg::Ready(b)) => {
                    let (info, file) = *b;
                    self.phase = Phase::Ready { info, file };
                    self.rx = None;
                    return;
                }
                Ok(Msg::Failed(e)) => {
                    self.phase = Phase::Failed(e);
                    self.rx = None;
                    return;
                }
                Err(TryRecvError::Empty) => {
                    // 只是「还没有新消息」。按阶段决定下一次醒点：
                    // - 下载中：界面上有个百分比在变，按 PROGRESS_BEAT 醒；
                    // - 检查中：界面上**什么都没变**（纯网络往返），不该重绘，
                    //   走 IDLE_BEAT 兜底即可 —— 结果到达时线程会主动叫醒我们。
                    // 此前一律 120ms，等于整个检查+下载期间界面都被钉在 8fps。
                    ctx.request_repaint_after(match self.phase {
                        Phase::Downloading { .. } => PROGRESS_BEAT,
                        _ => IDLE_BEAT,
                    });
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.phase = Phase::Failed("更新线程意外中断".into());
                    self.rx = None;
                    return;
                }
            }
        }
    }
}

/// 检查更新（阻塞）。返回 `Ok(None)` 表示确实已是最新。
///
/// 演示源直接给一份固定的「有新版」数据，但**仍然走同一套本地重算**
/// （`build` 不比本机大就当没更新）—— 演示要演的是真实逻辑，不是绕过它。
fn check_blocking(source: &Source) -> Result<Option<ReleaseInfo>, String> {
    match source {
        Source::Server(p) => check_server(p),
        Source::Github(g) => crate::github::check(g),
        Source::Mock => {
            let info = crate::mock::latest_release();
            Ok((info.build > my_build()).then_some(info))
        }
    }
}

/// 下载 + 三重校验（阻塞）。返回可执行的安装包路径。
///
/// 演示源只把进度跑完、不产生任何文件；它的返回路径也永远不会被执行
/// （`Source::allows_install()` 为 false）。
fn download_blocking(
    source: &Source,
    info: &ReleaseInfo,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, String> {
    match source {
        Source::Server(p) => download_server(p, info, on_progress),
        Source::Github(g) => crate::github::download(g, info, on_progress),
        Source::Mock => Ok(crate::mock::download_release(info, on_progress)),
    }
}

fn check_server(profile: &ServerProfile) -> Result<Option<ReleaseInfo>, String> {
    let (platform, arch) = platform_arch()?;
    let channel = "stable";
    let path = format!(
        "/version/latest?channel={channel}&platform={platform}&arch={arch}&current_version={}&current_build={}",
        my_version(),
        my_build()
    );
    let v = net::call(profile, "GET", &path, None).map_err(|e| e.to_string())?;

    // 回显校验：query 在信封之外可被改写，服务端把它们原样回显，这里逐字比对。
    // 不一致说明请求在路上被动过手脚。
    let echo = |k: &str, want: &str| -> Result<(), String> {
        match v.get(k).and_then(|x| x.as_str()) {
            Some(got) if got == want => Ok(()),
            got => Err(format!(
                "请求参数疑似被篡改：{k} 发出的是 {want}，服务端回显 {got:?}"
            )),
        }
    };
    echo("channel", channel)?;
    echo("platform", platform)?;
    echo("arch", arch)?;

    let Some(l) = v.get("latest").filter(|x| !x.is_null()) else {
        return Ok(None); // 该组合下还没发布过
    };
    let s = |k: &str| l.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
    let n = |k: &str| l.get(k).and_then(|x| x.as_i64()).unwrap_or(0);

    let info = ReleaseInfo {
        id: n("id"),
        version: s("version"),
        build: n("build"),
        notes: s("notes"),
        sha256: s("sha256"),
        size: n("size"),
        min_supported_build: n("min_supported_build"),
        signature: s("signature"),
        ext: s("ext"),
        force: false,
    };
    if info.id == 0 || info.version.is_empty() || info.sha256.is_empty() || info.size <= 0 {
        return Err("服务端返回的版本信息不完整".into());
    }

    // 本地重算，不采信服务端的 has_update / force
    if info.build <= my_build() {
        return Ok(None);
    }
    // 阈值配歪了（超过最新 build）视为配置错误，不强制 —— 否则会把用户困在
    // 「必须更新到一个不存在的版本」
    let force = my_build() < info.min_supported_build && info.min_supported_build <= info.build;
    Ok(Some(ReleaseInfo { force, ..info }))
}

/// 每个平台允许的安装包扩展名。**不从明文 `Content-Disposition` 取扩展名** ——
/// 那个头中间人可改写，而扩展名决定了我们怎么执行这个文件。
pub(crate) fn ext_allowed(platform: &str, ext: &str) -> bool {
    let allow: &[&str] = match platform {
        "windows" => &[".exe", ".msi"],
        "macos" => &[".dmg", ".pkg"],
        "linux" => &[".AppImage", ".deb"],
        _ => &[],
    };
    allow.iter().any(|a| a.eq_ignore_ascii_case(ext))
}

/// 按扩展名做魔数自检。格式不确定的（dmg/pkg/msi）跳过，不假装能查。
fn magic_ok(ext: &str, head: &[u8]) -> bool {
    let starts = |m: &[u8]| head.len() >= m.len() && &head[..m.len()] == m;
    match ext.to_ascii_lowercase().as_str() {
        ".exe" => starts(b"MZ"),
        ".appimage" => starts(b"\x7fELF"),
        ".deb" => starts(b"!<arch>"),
        _ => true,
    }
}

fn download_server(
    profile: &ServerProfile,
    info: &ReleaseInfo,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let (platform, arch) = platform_arch()?;

    if info.signature.trim().is_empty() {
        return Err(
            "该版本未签名，已拒绝下载（签名是防止服务器被拿下后推恶意更新的唯一手段）".into(),
        );
    }
    let Some(verify_pk) = release::builtin_pubkey() else {
        return Err("本构建未烘入发布验签公钥，无法验证安装包来源".into());
    };
    if !ext_allowed(platform, &info.ext) {
        return Err(format!("不接受的安装包类型：{}", info.ext));
    }

    // 换一次性票据（走加密信道）
    let t = net::call(
        profile,
        "POST",
        "/download-ticket",
        Some(&serde_json::json!({ "kind": "app", "id": info.id })),
    )
    .map_err(|e| e.to_string())?;
    let url = t
        .get("url")
        .and_then(|x| x.as_str())
        .ok_or("换取下载票据失败")?;
    // 只当路径用，绝不接受指向别的 host 的绝对 URL
    if !url.starts_with('/') {
        return Err("服务端返回的下载地址非法".into());
    }
    // download_url 形如 /api/v1/xxx，而 base_url 已含 /api/v1，去掉重复前缀
    let path = url.strip_prefix("/api/v1").unwrap_or(url);

    // 落到 cache_dir 下**新建的随机名私有目录**。绝不用 /tmp —— 那是全局可写的，
    // 可预测的文件名等于邀请符号链接攻击；私有目录同时也挡住 Windows 安装程序
    // 从同目录侧加载恶意 DLL 的经典路径。
    let dir = fresh_update_dir()?;
    let file = dir.join(format!(
        "ferric-{}-{}-{}{}",
        info.version,
        info.build,
        &info.sha256[..info.sha256.len().min(16)],
        info.ext
    ));

    let total = info.size as u64;

    // 任何一条失败路径都要把这个私有目录带走。半截安装包留在盘上本身就是个
    // 可被替换的靶子 —— 原先只有「校验 / 验签不通过」会清，**网络中断那条直接
    // 返回**，几十 MB 的残包要一直躺到下次启动 `cleanup_stale` 才被收走。
    let fail = |dir: &Path, msg: String| -> String {
        let _ = std::fs::remove_dir_all(dir);
        msg
    };

    let done = match stream_to_file(profile, path, &file, total, on_progress) {
        Ok(d) => d,
        Err(e) => return Err(fail(&dir, e)),
    };
    on_progress(done, total);

    verify_downloaded(&dir, &file, info, platform, arch, verify_pk)?;
    Ok(file)
}

/// 落盘之后的三重校验：大小 + sha256 → 魔数 → 离线签名。
///
/// **每一条来源都必须走这一个函数**（服务端、GitHub 发布页……）。校验代码一旦被抄成
/// 两份，两份就会各自演化，而「哪一份漏了签名那一步」是没人会主动去查的。
///
/// 期望值（`info`）必须来自**可信信道**：自建服务端是加密信道里的清单，
/// GitHub 是验过签的 manifest。绝不能用下载响应头里的值 —— 那个能改内容的人也能改。
///
/// 任何一条不过都把整个暂存目录带走：半截安装包留在盘上本身就是个可被替换的靶子。
pub(crate) fn verify_downloaded(
    dir: &Path,
    file: &Path,
    info: &ReleaseInfo,
    platform: &str,
    arch: &str,
    verify_pk: &str,
) -> Result<(), String> {
    let fail = |dir: &Path, msg: String| -> String {
        let _ = std::fs::remove_dir_all(dir);
        msg
    };
    let total = info.size as u64;

    // **从磁盘重新读一遍算哈希** —— 要校验的必须是「将要被执行的那串字节」，
    // 而不是内存里的缓冲区。顺带能抓住写入期间的竞态与磁盘损坏。
    let mut f = match open_locked(file) {
        Ok(f) => f,
        Err(e) => return Err(fail(dir, e)),
    };
    let mut hasher = Sha256::new();
    let mut head = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size: u64 = 0;
    loop {
        let n = match f.read(&mut buf) {
            Ok(n) => n,
            // 先关句柄再删目录，否则 Windows 上删不掉（同上）
            Err(e) => {
                drop(f);
                return Err(fail(dir, format!("回读失败：{e}")));
            }
        };
        if n == 0 {
            break;
        }
        if head.len() < 8 {
            head.extend_from_slice(&buf[..n.min(8)]);
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let got = hex::encode(hasher.finalize());
    // 读完就关。下面每一条失败路径都要 `remove_dir_all`，而在 Windows 上，
    // 我们自己这个不许 DELETE 共享的句柄会把删除挡回来 —— 句柄不放，
    // 「失败即清理」就是句空话。往后的校验都只看已经读进内存的 got/size/head，
    // 不再碰这个文件，所以关掉不损失任何东西。
    drop(f);

    if size != total {
        return Err(fail(
            dir,
            format!("大小不符：期望 {total} 字节，实际 {size}"),
        ));
    }
    if !got.eq_ignore_ascii_case(&info.sha256) {
        return Err(fail(
            dir,
            "内容校验失败：sha256 与清单中声明的不一致".into(),
        ));
    }
    if !magic_ok(&info.ext, &head) {
        return Err(fail(dir, format!("文件格式与 {} 不符，已拒绝", info.ext)));
    }

    // 最后一道也是最重要的一道：离线签名。服务器被拿下也签不出这个。
    let payload = release::signing_payload(
        &info.version,
        info.build,
        platform,
        arch,
        &info.sha256,
        info.size,
    );
    if let Err(e) = release::verify(verify_pk, &info.signature, &payload) {
        return Err(fail(dir, e));
    }

    // 三重校验都过了才给可执行位（unix）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// 边下边写，返回落盘字节数。
///
/// 单独一个函数（而不是调用处的一个块）是为了让文件句柄在返回时**必然析构**：
/// 调用方在失败路径上紧接着要 `remove_dir_all` 整个暂存目录，而 Windows 上
/// 目录里还有打开的句柄时删不掉 —— 那样「失败即清理」就成了静默失效。
///
/// 这里**刻意不做**流式哈希：要校验的必须是「将要被执行的那串字节」，
/// 由调用方从磁盘重新读一遍算。原先这里挂着一个从不 finalize 的 Sha256，
/// 等于把几十 MB 白算了一遍。
fn stream_to_file(
    profile: &ServerProfile,
    path: &str,
    file: &Path,
    total: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, String> {
    // create_new：已存在的文件或符号链接直接失败，不跟随
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .map_err(|e| format!("创建临时文件失败：{e}"))?;
    let mut done: u64 = 0;
    let mut last_report = std::time::Instant::now();
    net::download_to(profile, path, total, &mut |chunk| {
        out.write_all(chunk)?;
        done += chunk.len() as u64;
        // 进度节流，别把 channel 塞爆
        if last_report.elapsed() >= std::time::Duration::from_millis(120) {
            on_progress(done, total);
            last_report = std::time::Instant::now();
        }
        Ok(())
    })
    .map_err(|e: NetErr| e.to_string())?;
    out.flush()
        .and_then(|_| out.sync_all())
        .map_err(|e| format!("落盘失败：{e}"))?;
    Ok(done)
}

/// 以拒绝写共享的方式打开（Windows）：回读校验期间没有第二个进程能往里写，
/// 「读到的字节」与「算出的哈希」因此必然是同一份。
///
/// ⚠️ 它**只覆盖回读这一段**。句柄一放，「已就绪 → 用户点安装」之间那个更长的
/// 窗口就没人守了 —— 这里不硬扛，是因为守它要把句柄一路攥到 `launch`，而
/// Windows 加载可执行映像时要求文件允许 DELETE 共享，攥着句柄很可能直接让
/// 安装程序起不来。真正兜住那一段的是**离线签名 + 私有随机目录**：
/// 目录名不可预测，攻击者连该往哪写都不知道。
fn open_locked(path: &Path) -> Result<std::fs::File, String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
            .map_err(|e| format!("回读打开失败：{e}"))
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(path).map_err(|e| format!("回读打开失败：{e}"))
    }
}

/// 更新包的暂存根目录。
fn updates_root() -> Option<PathBuf> {
    let pd = directories::ProjectDirs::from("", "", "ferric")?;
    Some(pd.cache_dir().join("updates"))
}

pub(crate) fn fresh_update_dir() -> Result<PathBuf, String> {
    use rand::RngCore;
    let root = updates_root().ok_or("无法定位缓存目录")?;
    std::fs::create_dir_all(&root).map_err(|e| format!("创建缓存目录失败：{e}"))?;
    // 随机名 + create_dir（已存在即失败），杜绝可预测路径
    for _ in 0..8 {
        let mut b = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut b);
        let dir = root.join(hex::encode(b));
        if std::fs::create_dir(&dir).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
            return Ok(dir);
        }
    }
    Err("创建更新暂存目录失败".into())
}

/// 启动时清理上次遗留的暂存目录 —— 留在盘上的旧安装包本身就是个可被替换的靶子。
pub fn cleanup_stale() {
    let Some(root) = updates_root() else { return };
    let Ok(rd) = std::fs::read_dir(&root) else {
        return;
    };
    for e in rd.flatten() {
        let _ = std::fs::remove_dir_all(e.path());
    }
}

/// 把安装包交给系统。**注意各平台行为并不一致**：
/// Windows 直接执行安装程序；macOS 用 `open` 挂载/打开（仍需用户拖拽或确认）；
/// Linux 的 .deb 需要 root，只能交给软件安装器。UI 文案要如实说明，别假装三平台一样。
pub fn launch(file: &Path) -> Result<(), String> {
    let dir = file.parent().ok_or("路径异常")?;
    let ext = file
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    let mut cmd = launch_command(std::env::consts::OS, &ext, file)?;
    // 工作目录设为那个私有目录，对抗 Windows 安装程序从当前目录侧加载 DLL
    cmd.current_dir(dir);
    cmd.spawn().map_err(|e| format!("启动安装程序失败：{e}"))?;
    Ok(())
}

/// 构造安装命令（纯函数，供单测检查参数）。
///
/// Windows 的 NSIS 安装器带 `/P`（passive：只显示进度条，跳过所有询问页，
/// 检测到旧版**直接覆盖**，不再弹「是否先卸载」）与 `/R`（装完自动重启应用）——
/// 应用内更新的体验应当是「点一下安装，装完 Ferric 自己回来」，中间零问题。
/// MSI 同理用 `/passive`。手动双击安装包的路径由定制 NSIS 模板负责同样的
/// 「升级即覆盖」语义（见 crates/ferric-app/nsis/installer.nsi 头注释）。
fn launch_command(os: &str, ext: &str, file: &Path) -> Result<std::process::Command, String> {
    Ok(match (os, ext) {
        ("windows", ".msi") => {
            let mut c = std::process::Command::new("msiexec");
            c.arg("/i").arg(file).arg("/passive");
            c
        }
        ("windows", _) => {
            let mut c = std::process::Command::new(file);
            c.arg("/P").arg("/R");
            c
        }
        ("macos", _) => {
            let mut c = std::process::Command::new("open");
            c.arg(file);
            c
        }
        ("linux", ".appimage") => std::process::Command::new(file),
        ("linux", _) => {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(file);
            c
        }
        _ => return Err("本平台不支持自动安装".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunUiExt;

    /// 扩展名白名单必须按平台严格限制 —— 它决定了我们怎么执行下载到的文件。
    #[test]
    fn ext_whitelist_is_per_platform() {
        assert!(ext_allowed("windows", ".exe"));
        assert!(ext_allowed("windows", ".msi"));
        assert!(!ext_allowed("windows", ".sh"), "不在白名单的一律拒绝");
        assert!(!ext_allowed("windows", ".dmg"), "别的平台的包也要拒绝");
        assert!(ext_allowed("linux", ".AppImage"));
        assert!(ext_allowed("linux", ".deb"));
        assert!(!ext_allowed("linux", ".exe"));
        assert!(ext_allowed("macos", ".dmg"));
        assert!(!ext_allowed("macos", ""), "空扩展名也要拒绝");
    }

    /// 应用内更新的 Windows 安装必须是 passive 覆盖模式：`/P` 跳过所有询问页
    ///（含「先卸载旧版」页，直接覆盖升级），`/R` 装完自动重启应用。
    /// 没有这两个参数，用户点「安装」还要再答一轮安装向导 —— 那不叫自动更新。
    #[test]
    fn windows_installer_runs_passive_with_relaunch() {
        let cmd = launch_command("windows", ".exe", Path::new("C:\\t\\setup.exe")).unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.contains(&"/P".to_owned()),
            "缺 /P（passive 覆盖）：{args:?}"
        );
        assert!(
            args.contains(&"/R".to_owned()),
            "缺 /R（装完重启）：{args:?}"
        );

        let msi = launch_command("windows", ".msi", Path::new("C:\\t\\a.msi")).unwrap();
        let margs: Vec<String> = msi
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            margs.contains(&"/passive".to_owned()),
            "MSI 缺 /passive：{margs:?}"
        );
    }

    /// 魔数与扩展名不符要能查出来（廉价地兜住「platform 被劫持」这类情况）。
    #[test]
    fn magic_matches_extension() {
        assert!(magic_ok(".exe", b"MZ\x90\x00"));
        assert!(!magic_ok(".exe", b"\x7fELF"), "ELF 冒充 exe 要被拒");
        assert!(magic_ok(".AppImage", b"\x7fELF\x02\x01"));
        assert!(!magic_ok(".AppImage", b"MZ\x90\x00"));
        assert!(magic_ok(".deb", b"!<arch>\n"));
        assert!(!magic_ok(".deb", b"MZ"));
        // 格式不确定的跳过检查，而不是假装能查
        assert!(magic_ok(".dmg", b"whatever"));
        assert!(magic_ok(".pkg", b"xx"));
        // 头部太短也不能 panic
        assert!(!magic_ok(".exe", b""));
        assert!(!magic_ok(".deb", b"!<"));
    }

    #[test]
    fn platform_arch_maps_to_server_whitelist() {
        // 当前构建平台必须能映射（否则本机根本跑不了自动更新）
        if let Ok((p, a)) = platform_arch() {
            assert!(["windows", "macos", "linux"].contains(&p));
            assert!(["x86_64", "aarch64"].contains(&a));
        }
    }

    /// 暂存目录必须**每次都是新的随机名**，且启动清理能把残包连目录一起带走。
    ///
    /// 可预测的路径等于邀请符号链接攻击；而下载中断后留在盘上的半截安装包
    /// 本身就是个可被替换的靶子 —— 两件事都靠这一对函数兜住。
    #[test]
    fn fresh_update_dir_is_unpredictable_and_cleanable() {
        let Ok(a) = fresh_update_dir() else {
            eprintln!("跳过：定位不到缓存目录");
            return;
        };
        let b = fresh_update_dir().expect("第二次也该建得出来");
        assert_ne!(a, b, "两次必须落在不同目录");
        assert!(a.is_dir() && b.is_dir());

        std::fs::write(a.join("half-downloaded.exe"), b"MZ").expect("写残包");
        cleanup_stale();
        assert!(!a.exists(), "启动清理必须把残包连目录一起带走");
        assert!(!b.exists());
    }

    #[test]
    fn my_build_is_parsable() {
        assert!(my_build() >= 0, "构建号解析不出来时必须回落到 0 而非 panic");
        assert!(!my_version().is_empty());
    }

    /// 把 egui 的时间轴推到 `t`（`tick` 里读的是上一帧的 `input().time`）。
    fn frame_at(ctx: &egui::Context, t: f64) {
        let _ = ctx.run_ui_cleared(
            egui::RawInput {
                time: Some(t),
                ..Default::default()
            },
            |_| {},
        );
    }

    fn custom_server() -> Source {
        Source::Server(ServerProfile {
            base_url: "http://127.0.0.1:1/api/v1".into(),
            pubkey: format!("04{}", "ab".repeat(64)),
        })
    }

    /// 关掉自动更新、或压根没有数据源时，后台流水线必须**一动不动**。
    ///
    /// 这条守的是「用户明确关掉了，程序却还在偷偷联网」——
    /// 检查更新会把本机版本号发给服务器，关了就必须是真的关了。
    #[test]
    fn auto_pipeline_stays_idle_when_disabled_or_sourceless() {
        let ctx = egui::Context::default();
        frame_at(&ctx, 100.0);

        let mut u = Updater::default();
        assert_eq!(
            u.tick(&ctx, Some(&Source::Mock), false, true),
            Tick::Nothing
        );
        assert!(matches!(u.phase, Phase::Idle), "关掉开关后仍然发起了检查");

        let mut u = Updater::default();
        assert!(matches!(u.tick(&ctx, None, true, true), Tick::Nothing));
        assert!(matches!(u.phase, Phase::Idle), "没有数据源却发起了检查");

        // 距上次检查还不够久 → 这一轮不查（跨启动节流）
        let mut u = Updater::default();
        assert_eq!(
            u.tick(&ctx, Some(&Source::Mock), true, false),
            Tick::Nothing
        );
        assert!(matches!(u.phase, Phase::Idle), "节流期内不该发起检查");
    }

    /// 自动检查要**延后**到启动之后一小会儿，别和首屏抢资源；到点了才真的查。
    #[test]
    fn auto_check_waits_a_moment_after_launch() {
        let ctx = egui::Context::default();
        frame_at(&ctx, 0.0);
        let mut u = Updater::default();

        u.tick(&ctx, Some(&Source::Mock), true, true);
        assert!(matches!(u.phase, Phase::Idle), "刚启动就查了，太急");
        assert_eq!(
            u.auto_check_at,
            Some(FIRST_CHECK_DELAY_SECS),
            "没有排定检查时刻，这次检查将永远不会发生"
        );

        // 时间推过去 → 应当真的发起检查
        frame_at(&ctx, FIRST_CHECK_DELAY_SECS + 1.0);
        u.tick(&ctx, Some(&Source::Mock), true, true);
        assert!(matches!(u.phase, Phase::Checking), "到点了却没发起检查");
    }

    /// 发现新版之后自动转入后台下载 —— 这是「点安装就能更新」的前提：
    /// 用户点的时候东西已经在盘上了，不必现等几十 MB。
    #[test]
    fn a_found_update_starts_downloading_in_the_background() {
        let ctx = egui::Context::default();
        frame_at(&ctx, 100.0);
        let mut u = Updater {
            phase: Phase::Available(crate::mock::latest_release()),
            ..Default::default()
        };
        u.tick(&ctx, Some(&Source::Mock), true, false);
        assert!(
            matches!(u.phase, Phase::Downloading { .. }),
            "发现新版却没有开始后台下载：{:?}",
            u.phase
        );

        // 一次运行只自动下一次：失败了也不再自动重试，免得变成流量灾难
        let mut u2 = Updater {
            phase: Phase::Available(crate::mock::latest_release()),
            auto_downloaded: true,
            ..Default::default()
        };
        u2.tick(&ctx, Some(&Source::Mock), true, false);
        assert!(
            matches!(u2.phase, Phase::Available(_)),
            "自动下载重复触发了"
        );
    }

    /// 自定义更新源**不许**后台自动下载 —— 那个地址可能是用户被诱导改的，
    /// 让它在后台往盘上拉东西是不可接受的（手动按钮仍然可用，且不会自动安装）。
    #[test]
    fn a_custom_source_never_downloads_by_itself() {
        let ctx = egui::Context::default();
        frame_at(&ctx, 100.0);
        let src = custom_server();
        assert!(!src.allows_auto_download());

        let mut u = Updater {
            phase: Phase::Available(crate::mock::latest_release()),
            ..Default::default()
        };
        u.tick(&ctx, Some(&src), true, false);
        assert!(
            matches!(u.phase, Phase::Available(_)),
            "自定义源竟然自动下载了：{:?}",
            u.phase
        );
    }

    /// 就绪只通知一次，不能每帧弹一遍。
    #[test]
    fn ready_notifies_exactly_once() {
        let ctx = egui::Context::default();
        frame_at(&ctx, 100.0);
        let mut u = Updater {
            phase: Phase::Ready {
                info: crate::mock::latest_release(),
                file: PathBuf::from("x"),
            },
            ..Default::default()
        };
        assert!(matches!(
            u.tick(&ctx, Some(&Source::Mock), true, false),
            Tick::ReadyToInstall { .. }
        ));
        for _ in 0..5 {
            assert!(
                matches!(
                    u.tick(&ctx, Some(&Source::Mock), true, false),
                    Tick::Nothing
                ),
                "已就绪的提示重复弹了"
            );
        }
    }

    /// 演示源必须能走完「有更新」的判定 —— 否则整条演示流程第一步就断了。
    #[test]
    fn mock_source_reports_an_update() {
        let r = check_blocking(&Source::Mock).expect("演示源检查不该失败");
        let info = r.expect("演示源应当报告有新版");
        assert!(info.build > my_build());
    }

    /// 版本串的形状合同：恰好三段、每段纯数字（UI 显示、更新服务器、安装包
    /// 元数据三方都按这个形状消费）。来源是发布 tag 或 Cargo.toml 占位，
    /// 哪边塞进来一个不合形状的值，这里立刻红。
    #[test]
    fn version_has_three_numeric_parts() {
        let v = my_version();
        let parts: Vec<&str> = v.split('.').collect();
        assert_eq!(parts.len(), 3, "版本应恰好三段：{v}");
        for p in parts {
            p.parse::<u64>()
                .unwrap_or_else(|_| panic!("段「{p}」不是数字：{v}"));
        }
    }
}

/// 对着**真实运行的服务端**跑一遍完整信任链。
///
/// 默认不跑（CI 里没有服务端）。需要三个编译期烘入值 + `FERRIC_E2E=1` 才会执行：
///
/// ```text
/// FERRIC_E2E=1 \
/// FERRIC_SERVER_URL=http://127.0.0.1:8700/api/v1 \
/// FERRIC_SERVER_PUBKEY=04… FERRIC_RELEASE_PUBKEY=04… \
///   cargo test -p ferric-ui e2e_full_chain -- --nocapture
/// ```
#[cfg(test)]
mod e2e {
    use super::*;

    #[test]
    fn e2e_full_chain() {
        if std::env::var("FERRIC_E2E").as_deref() != Ok("1") {
            eprintln!("跳过：未设置 FERRIC_E2E=1");
            return;
        }
        let profile = ServerProfile::builtin().expect("需要烘入 FERRIC_SERVER_URL / PUBKEY");
        profile.validate().expect("烘入的服务器配置非法");
        eprintln!("服务器  : {}", profile.base_url);
        eprintln!("公钥指纹: {}", profile.fingerprint());
        eprintln!("本机版本: v{} build {}", my_version(), my_build());

        let info = check_server(&profile)
            .expect("检查更新失败")
            .expect("服务端应有比本机更新的版本");
        eprintln!(
            "发现新版: v{} build {} size {} ext {}",
            info.version, info.build, info.size, info.ext
        );
        assert!(info.build > my_build(), "必须严格更新才算有更新");
        assert!(!info.signature.is_empty(), "服务端必须下发离线签名");
        assert!(!info.ext.is_empty(), "扩展名必须走加密信道下发");

        let file = download_server(&profile, &info, &mut |d, t| {
            if t > 0 && d == t {
                eprintln!("下载完成: {d}/{t}");
            }
        })
        .expect("下载 / 校验 / 验签失败");
        eprintln!("已就绪  : {}", file.display());

        // 落盘内容必须与服务端在加密信道里声明的 sha256 一致
        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(bytes.len() as i64, info.size);
        assert_eq!(hex::encode(Sha256::digest(&bytes)), info.sha256);

        // 签名换成别的版本的清单必须验不过（防止拿 A 版签名冒充 B 版）
        let (p, a) = platform_arch().unwrap();
        let wrong =
            release::signing_payload("9.9.9", info.build + 1, p, a, &info.sha256, info.size);
        assert!(
            release::verify(release::builtin_pubkey().unwrap(), &info.signature, &wrong).is_err(),
            "签名必须绑定到具体版本"
        );

        let _ = std::fs::remove_dir_all(file.parent().unwrap());
        eprintln!("全链路通过");
    }
}

/// 攻击面回归测试：用一个**假服务端**模拟中间人能做的事，断言客户端不上当。
///
/// 这些是本轮设计真正的价值所在 —— 每一条都对应一种「校验全部通过但结论是错的」
/// 的攻击。没有它们，前面那些守卫很容易在后续重构里被悄悄削掉。
#[cfg(test)]
mod attacks {
    use super::*;
    use libsm::sm2::encrypt::DecryptCtx;
    use libsm::sm2::signature::SigCtx;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;

    /// 起一个假服务端：用自己的密钥解开客户端的会话密钥，然后按 `make_body`
    /// 生成响应。`encrypt=false` 时故意返回明文（模拟降级攻击）。
    fn fake_server(
        encrypt: bool,
        make_body: impl Fn() -> serde_json::Value + Send + 'static,
    ) -> (String, String) {
        let ctx = SigCtx::new();
        let (pk, sk) = ctx.new_keypair().unwrap();
        let pk_hex = hex::encode(ctx.serialize_pubkey(&pk, false).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        std::thread::spawn(move || {
            for stream in listener.incoming().take(1) {
                let Ok(mut s) = stream else { continue };
                // 读请求头，取出 X-Enc-Key
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut key_hdr = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("x-enc-key:") {
                        key_hdr = v.trim().to_owned();
                    }
                }
                let body = make_body().to_string();
                let resp = if encrypt && !key_hdr.is_empty() {
                    let ct = hex::decode(&key_hdr).unwrap();
                    let session = DecryptCtx::new(ct.len() - 97, sk.clone())
                        .decrypt(&ct)
                        .unwrap();
                    let mut iv = [0u8; 16];
                    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut iv);
                    let out = libsm::sm4::Cipher::new(&session, libsm::sm4::Mode::Gcm)
                        .unwrap()
                        .encrypt(&[], body.as_bytes(), &iv)
                        .unwrap();
                    let (d, t) = out.split_at(out.len() - 16);
                    let env =
                        serde_json::json!({"d": hex::encode(d), "t": hex::encode(t)}).to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Enc: sm2-sm4-gcm\r\nX-Enc-Iv: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        hex::encode(iv), env.len(), env
                    )
                } else {
                    // 明文响应 —— 模拟中间人剥掉加密层
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    )
                };
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
                // 不主动 shutdown(Write)：响应带 Content-Length，客户端读完 body 会主动
                // 关闭连接（发 FIN），服务端读到 EOF 再退；读超时兜底，避免异常时永久阻塞。
                let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut sink = [0u8; 512];
                while let Ok(n) = s.read(&mut sink) {
                    if n == 0 {
                        break;
                    }
                }
            }
        });
        (format!("http://{addr}"), pk_hex)
    }

    fn latest_json(build: i64, has_update: bool, platform: &str) -> serde_json::Value {
        serde_json::json!({
            "has_update": has_update, "force": false,
            "channel": "stable", "platform": platform, "arch": std::env::consts::ARCH,
            "latest": {
                "id": 1, "version": "9.9.9", "build": build, "notes": "",
                "sha256": "aa".repeat(32), "size": 10, "min_supported_build": 0,
                "signature": "3045", "ext": ".AppImage",
                "download_url": "/api/v1/version/1/download"
            }
        })
    }

    /// 攻击一：中间人改写 query 里的 `current_build`，让服务端「正确地」算出
    /// `has_update=false`。客户端**本地重算**，必须依然发现新版本。
    #[test]
    fn rewritten_current_build_cannot_suppress_updates() {
        let newer = my_build() + 100;
        let (url, pk) = fake_server(true, move || {
            latest_json(
                newer,
                /* 服务端被骗后说没更新 */ false,
                std::env::consts::OS,
            )
        });
        let p = ServerProfile {
            base_url: url,
            pubkey: pk,
        };
        let r = check_server(&p).expect("检查不应失败");
        assert!(
            r.is_some(),
            "服务端说 has_update=false，但本地重算 build 更大，必须仍报告有更新"
        );
        assert_eq!(r.unwrap().build, newer);
    }

    /// 攻击二：中间人改写 `platform`，让客户端拿到别的平台的包。
    /// 服务端会原样回显被改后的值，客户端逐字比对即可发现。
    #[test]
    fn mismatched_echo_is_treated_as_tampering() {
        let wrong = if std::env::consts::OS == "linux" {
            "windows"
        } else {
            "linux"
        };
        let newer = my_build() + 1;
        let (url, pk) = fake_server(true, move || latest_json(newer, true, wrong));
        let p = ServerProfile {
            base_url: url,
            pubkey: pk,
        };
        let e = check_server(&p).expect_err("回显不符必须判定为攻击");
        assert!(e.contains("篡改"), "错误信息应指出被篡改：{e}");
    }

    /// 攻击三：中间人把加密层整个剥掉，回一个明文的「已是最新」。
    /// 客户端必须归类为**检查失败**，绝不能显示成「已是最新版本」。
    #[test]
    fn plaintext_response_is_failure_not_up_to_date() {
        let (url, pk) = fake_server(false, || {
            serde_json::json!({
                "has_update": false, "force": false,
                "channel": "stable", "platform": std::env::consts::OS,
                "arch": std::env::consts::ARCH, "latest": null
            })
        });
        let p = ServerProfile {
            base_url: url,
            pubkey: pk,
        };
        let e = check_server(&p).expect_err("明文响应必须失败");
        assert!(
            e.contains("加密协议") || e.contains("中间人"),
            "必须明确是加密协议问题，而不是被当成「已是最新」：{e}"
        );
    }
}
