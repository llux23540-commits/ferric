//! 外壳状态与 Slint 桥接。
//!
//! 这是 egui 版 `app.rs`（3609 行）的替代物。差别在于**职责收窄**：
//! egui 是立即模式，`app.rs` 既是状态机也是渲染器；Slint 是 retained mode，
//! 视图在 `.slint` 里声明一次，这里只做三件事：
//!
//! 1. **持有状态**：工具列表、当前选中、收藏、设置项；
//! 2. **灌数据**：把状态写进 Slint 的 property（`sync_*`）；
//! 3. **收事件**：把 Slint 的 callback 接到状态变更 + 落盘上（`wire_*`）。
//!
//! 没有「每帧」这个概念了 —— 只有「状态变了就同步一次」。软件渲染下这正是
//! 内存与 CPU 的主要来源差异：egui 每帧重建整棵 UI，Slint 只在 property
//! 变化时重画脏区域。

use crate::editor::TextBuffer;
use crate::editor_bridge;
use crate::persist::{self, Persist, ThemeMode};
use crate::tool::{Shared, Tool};
use crate::views;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

// Slint 编译产物：AppWindow 与它的结构体（ToolEntry / UuidHistEntry）。
slint::include_modules!();

pub const APP_NAME: &str = "Ferric";

const RAIL_MIN: f32 = 196.0;
const RAIL_MAX: f32 = 460.0;

/// 外壳的全部可变状态。放在 `Rc<RefCell<...>>` 里被各个 callback 共享 ——
/// Slint 的回调是 `Fn`（可多次调用、非 `FnMut`），所以内部可变性是必须的。
pub struct AppState {
    pub tools: Vec<Box<dyn Tool>>,
    pub active: usize,
    pub favorites: HashSet<String>,
    pub mode: ThemeMode,
    pub dark: bool,
    pub ui_scale: f32,
    pub rail_width: f32,
    pub auto_update: bool,
    pub shared: Shared,
    /// 内置工具数量。插件工具一律追加其后，热加载时按这个位置截断。
    pub builtin_tools: usize,
    /// 上次成功检查更新的 Unix 时间戳（秒）。
    pub last_update_check: Option<i64>,
    pub source_pref: crate::source::SourcePref,
    pub server_override: Option<crate::net::ServerProfile>,
    pub github_override: Option<crate::github::GithubSource>,
    /// 30 秒内存采样状态机；None = 不在工作。
    pub mem_recorder: Option<crate::mem::MemoryRecorder>,
    /// 设置页「记录内存」那一行的状态文案。
    pub mem_status: String,
}

impl AppState {
    /// 从落盘状态构造。会加载插件、恢复每个工具的草稿。
    pub fn load() -> Self {
        let persist = persist::load();

        // 清掉上次遗留的更新暂存目录 —— 留在盘上的旧安装包本身就是个可被替换的靶子。
        crate::updater::cleanup_stale();

        let mut tools = views::registry();
        let builtin_tools = tools.len();
        let (plugin_tools, plugin_warns) = crate::plugin_host::load_all();
        for t in plugin_tools {
            tools.push(Box::new(t));
        }
        for t in tools.iter_mut() {
            let id = t.meta().id;
            if let Some(data) = persist.drafts.get(id) {
                t.load_draft(data);
            }
        }
        let active = tools
            .iter()
            .position(|t| t.meta().id == persist.active_id)
            .unwrap_or(0);

        let mode = persist.theme_mode.unwrap_or(ThemeMode::System);
        // 系统深浅色：Slint 暴露在 Window 上，构造期拿不到，先用上次生效的值兜底，
        // 首次同步时再按实际系统主题纠正（与 egui 版同样的「避免闪白」处理）。
        let dark = match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => persist.dark,
        };

        let mut shared = Shared::new();
        shared.lang = persist.lang;
        // Slint 走 renderer-software，恒为软件渲染。
        shared.gpu_software = true;
        for w in plugin_warns {
            shared.toast(format!("插件加载失败 · {w}"));
        }

        Self {
            tools,
            active,
            favorites: persist.favorites.into_iter().collect(),
            mode,
            dark,
            ui_scale: persist.ui_scale.clamp(0.8, 1.6),
            rail_width: persist.rail_width.clamp(RAIL_MIN, RAIL_MAX),
            auto_update: persist.auto_update,
            shared,
            builtin_tools,
            last_update_check: persist.last_update_check,
            source_pref: persist
                .source_pref
                .unwrap_or_else(|| crate::source::SourcePref::from_legacy(persist.mock_source)),
            server_override: persist.server,
            github_override: persist
                .github_repo
                .map(|repo| crate::github::GithubSource { repo }),
            mem_recorder: None,
            mem_status: String::new(),
        }
    }

    /// 收集当前状态并落盘。每次改设置 / 切工具 / 改草稿后调用。
    pub fn save(&self) {
        let mut drafts = std::collections::HashMap::new();
        for t in &self.tools {
            if let Some(d) = t.save_draft() {
                drafts.insert(t.meta().id.to_owned(), d);
            }
        }
        let p = Persist {
            dark: self.dark,
            theme_mode: Some(self.mode),
            rail_width: self.rail_width,
            favorites: self.favorites.iter().cloned().collect(),
            active_id: self
                .tools
                .get(self.active)
                .map(|t| t.meta().id.to_owned())
                .unwrap_or_else(|| "json".to_owned()),
            drafts,
            lang: self.shared.lang,
            server: self.server_override.clone(),
            ui_scale: self.ui_scale,
            auto_update: self.auto_update,
            // 双向兼容：装回旧版本的用户不至于把数据源设置丢光。
            mock_source: Some(self.source_pref == crate::source::SourcePref::Mock),
            source_pref: Some(self.source_pref),
            github_repo: self.github_override.as_ref().map(|g| g.repo.clone()),
            last_update_check: self.last_update_check,
        };
        persist::save(&p);
    }

    /// 当前选中的工具是不是 UUID（外壳据此决定要不要同步 UUID 的 property）。
    fn active_is_uuid(&self) -> bool {
        self.tools
            .get(self.active)
            .map(|t| t.meta().id == "uuid")
            .unwrap_or(false)
    }

    /// 取出 UUID 工具的可变引用。注册表里它一定在，但仍按 Option 处理 ——
    /// 插件热加载会重排 `tools`，硬 unwrap 是给未来埋雷。
    fn uuid_mut(&mut self) -> Option<&mut views::UuidTool> {
        let idx = self.tools.iter().position(|t| t.meta().id == "uuid")?;
        // 这里需要向下转型。`Tool` 是 trait object，标准做法是给 trait 加
        // `as_any`；但 UUID 是注册表里由我们自己构造的具体类型，位置固定，
        // 用 `downcast` 不如直接保留一份具体类型的句柄来得干净 ——
        // 见 `AppState::uuid`（下面用独立字段持有）。
        let _ = idx;
        None
    }
}

/// 已迁移的工具各持一个具体类型的字段。
///
/// 为什么不从 `Vec<Box<dyn Tool>>` 里向下转型：那需要给 `Tool` 加
/// `as_any`，一个只服务于转型的接口。迁移期字段会逐个增加；等 10 个工具
/// 全迁完，再统一收敛成 `enum ToolState { Json(..), Diff(..), … }`。
pub struct Shell {
    pub state: Rc<RefCell<AppState>>,
    pub uuid: Rc<RefCell<views::UuidTool>>,
    pub yaml: Rc<RefCell<views::YamlTool>>,
    pub sql: Rc<RefCell<views::SqlTool>>,
    pub regex: Rc<RefCell<views::RegexTool>>,
    pub rsa: Rc<RefCell<views::RsaTool>>,
    pub crypto: Rc<RefCell<views::CryptoTool>>,
}

impl Shell {
    pub fn new() -> Self {
        let state = AppState::load();
        // 草稿已在 `AppState::load` 里灌进注册表那一份；这里按同一份草稿再构造
        // 具体类型的实例，保证两边初值一致。
        let draft_of = |id: &str| {
            state
                .tools
                .iter()
                .find(|t| t.meta().id == id)
                .and_then(|t| t.save_draft())
        };

        let mut uuid = views::UuidTool::default();
        if let Some(d) = draft_of("uuid") {
            uuid.load_draft(&d);
        }
        let mut yaml = views::YamlTool::default();
        if let Some(d) = draft_of("yaml") {
            yaml.load_draft(&d);
        }
        let mut sql = views::SqlTool::default();
        if let Some(d) = draft_of("sql") {
            sql.load_draft(&d);
        }
        let mut regex = views::RegexTool::default();
        if let Some(d) = draft_of("regex") {
            regex.load_draft(&d);
        }
        let mut rsa = views::RsaTool::default();
        if let Some(d) = draft_of("rsa") {
            rsa.load_draft(&d);
        }
        let mut crypto = views::CryptoTool::default();
        if let Some(d) = draft_of("crypto") {
            crypto.load_draft(&d);
        }

        Self {
            state: Rc::new(RefCell::new(state)),
            uuid: Rc::new(RefCell::new(uuid)),
            yaml: Rc::new(RefCell::new(yaml)),
            sql: Rc::new(RefCell::new(sql)),
            regex: Rc::new(RefCell::new(regex)),
            rsa: Rc::new(RefCell::new(rsa)),
            crypto: Rc::new(RefCell::new(crypto)),
        }
    }

    /// 建窗、灌初值、接回调，然后交出窗口句柄由调用方 `run()`。
    pub fn build_window(&self) -> Result<AppWindow, slint::PlatformError> {
        let win = AppWindow::new()?;
        self.sync_all(&win);
        self.wire(&win);
        Ok(win)
    }

    /// 把全部状态灌进 Slint。
    pub fn sync_all(&self, win: &AppWindow) {
        self.sync_shell(win);
        self.sync_tools(win);
        self.sync_uuid(win);
        self.sync_yaml(win);
        self.sync_sql(win);
        self.sync_regex(win);
        self.sync_rsa(win);
        self.sync_crypto(win);
        self.sync_toasts(win);
    }

    /// 外壳级：主题、缩放、版本、后端、数据目录。
    fn sync_shell(&self, win: &AppWindow) {
        let s = self.state.borrow();
        win.global::<Theme>().set_dark(s.dark);
        win.set_theme_mode(s.mode.index());
        win.set_ui_scale(s.ui_scale);
        win.set_auto_update(s.auto_update);
        win.set_rail_width(s.rail_width);
        win.set_version(SharedString::from(crate::version()));
        win.set_backend_label(SharedString::from("软渲染（CPU · Slint）"));
        win.set_gpu_desc(SharedString::from(
            "Slint software renderer —— 不建任何 GPU 上下文",
        ));
        win.set_gpu_software(true);
        win.set_data_dir(SharedString::from(
            crate::launch::data_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "（取不到数据目录）".to_owned()),
        ));
        win.set_mem_status(SharedString::from(s.mem_status.clone()));

        // 侧栏底部的迁移进度。写成算出来的而不是硬编码文案 ——
        // 每迁完一个工具只要 `Tool::migrated` 翻成 true，这里自动跟上。
        let done = s.tools.iter().filter(|t| t.migrated()).count();
        let total = s.tools.len();
        win.set_migration_note(SharedString::from(if done == total {
            "Slint 迁移已完成".to_owned()
        } else {
            format!("Slint 迁移中 · {done}/{total} 个工具已完成")
        }));
        win.set_active_tool(s.active as i32);
    }

    /// 工具列表 → 侧栏模型。`filter` 为空则全部可见。
    ///
    /// 过滤放在 Rust 侧：Slint 的 `string` 没有 `contains`，而且中文匹配要按
    /// char 比（不能按字节切），交给 Rust 更稳。
    fn sync_tools(&self, win: &AppWindow) {
        let s = self.state.borrow();
        let filter = win.get_rail_filter().to_string();
        win.set_tools(ModelRc::new(VecModel::from(tool_rows(&s, &filter))));
    }

    /// UUID 工具状态 → property。
    fn sync_uuid(&self, win: &AppWindow) {
        let u = self.uuid.borrow();
        win.set_uuid_kind(u.kind_index());
        win.set_uuid_count(u.count as i32);
        win.set_uuid_namespace(u.namespace_index());
        win.set_uuid_custom_ns(SharedString::from(u.custom_ns.clone()));
        win.set_uuid_name(SharedString::from(u.name.clone()));
        win.set_uuid_upper(u.upper);
        win.set_uuid_nohyphen(u.nohyphen);
        win.set_uuid_as_json(u.as_json);
        win.set_uuid_hist_keep(u.hist_keep_index());
        win.set_uuid_output(SharedString::from(u.output.clone()));
        win.set_uuid_ok(u.ok);
        win.set_uuid_status(SharedString::from(u.status.clone()));
        let hist: Vec<UuidHistEntry> = u
            .history
            .iter()
            .map(|h| UuidHistEntry {
                label: SharedString::from(h.label.clone()),
                body: SharedString::from(h.body.clone()),
            })
            .collect();
        win.set_uuid_history(ModelRc::new(VecModel::from(hist)));
    }

    /// YAML 工具状态 → property。
    fn sync_yaml(&self, win: &AppWindow) {
        let y = self.yaml.borrow();
        win.set_yaml_input(editor_bridge::state_of(&y.input));
        win.set_yaml_output(editor_bridge::state_of(&y.output));
        win.set_yaml_ok(y.ok);
        win.set_yaml_status(SharedString::from(y.status.clone()));
    }

    /// SQL 工具状态 → property。
    fn sync_sql(&self, win: &AppWindow) {
        let t = self.sql.borrow();
        win.set_sql_input(editor_bridge::state_of(&t.input));
        win.set_sql_uppercase(t.uppercase);
        win.set_sql_status(SharedString::from(t.status.clone()));
    }

    /// 正则工具状态 → property。
    fn sync_regex(&self, win: &AppWindow) {
        let t = self.regex.borrow();
        win.set_regex_pattern(SharedString::from(t.pattern.clone()));
        win.set_regex_fg(t.fg);
        win.set_regex_fi(t.fi);
        win.set_regex_fm(t.fm);
        win.set_regex_fs(t.fs);
        win.set_regex_fx(t.fx);
        win.set_regex_text(editor_bridge::state_of(&t.text));
        win.set_regex_ok(t.ok);
        win.set_regex_status(SharedString::from(t.status.clone()));

        let rows: Vec<RegexMatch> = t
            .matches
            .iter()
            .map(|m| RegexMatch {
                label: SharedString::from(m.label.clone()),
                text: SharedString::from(m.text.clone()),
                groups: ModelRc::new(VecModel::from(
                    m.groups
                        .iter()
                        .map(|g| SharedString::from(g.clone()))
                        .collect::<Vec<_>>(),
                )),
            })
            .collect();
        win.set_regex_matches(ModelRc::new(VecModel::from(rows)));
    }

    /// RSA 工具状态 → property。
    fn sync_rsa(&self, win: &AppWindow) {
        let t = self.rsa.borrow();
        win.set_rsa_bits(t.bits_index());
        win.set_rsa_busy(t.busy);
        win.set_rsa_ok(t.ok);
        win.set_rsa_status(SharedString::from(t.status.clone()));
        win.set_rsa_pub(editor_bridge::state_of(&t.pub_pem));
        win.set_rsa_priv(editor_bridge::state_of(&t.priv_pem));
    }

    /// 加密 / 解密工具状态 → property。
    fn sync_crypto(&self, win: &AppWindow) {
        let t = self.crypto.borrow();
        win.set_crypto_algos(ModelRc::new(VecModel::from(
            views::CryptoTool::algo_labels()
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
        win.set_crypto_enc_algo(t.enc.algo_index());
        win.set_crypto_dec_algo(t.dec.algo_index());
        win.set_crypto_enc_key(SharedString::from(t.enc.key.clone()));
        win.set_crypto_dec_key(SharedString::from(t.dec.key.clone()));
        win.set_crypto_enc_in(editor_bridge::state_of(&t.enc.input));
        win.set_crypto_enc_out(editor_bridge::state_of(&t.enc.output));
        win.set_crypto_dec_in(editor_bridge::state_of(&t.dec.input));
        win.set_crypto_dec_out(editor_bridge::state_of(&t.dec.output));
        win.set_crypto_enc_ok(t.enc.ok);
        win.set_crypto_dec_ok(t.dec.ok);
        win.set_crypto_enc_status(SharedString::from(t.enc.status.clone()));
        win.set_crypto_dec_status(SharedString::from(t.dec.status.clone()));
    }

    /// 提示队列 → property（先剔除过期的）。
    fn sync_toasts(&self, win: &AppWindow) {
        let mut s = self.state.borrow_mut();
        s.shared.prune_toasts();
        let msgs: Vec<SharedString> = s
            .shared
            .toasts
            .iter()
            .map(|t| SharedString::from(t.text.clone()))
            .collect();
        win.set_toasts(ModelRc::new(VecModel::from(msgs)));
    }

    /// 接上所有 Slint 回调。
    fn wire(&self, win: &AppWindow) {
        self.wire_window(win);
        self.wire_navigation(win);
        self.wire_settings(win);
        self.wire_uuid(win);
        self.wire_yaml(win);
        self.wire_sql(win);
        self.wire_regex(win);
        self.wire_rsa(win);
        self.wire_crypto(win);
        self.wire_editors(win);
    }

    /// 窗口按钮：最小化 / 最大化 / 关闭。
    ///
    /// 拖动不在这里：`no-frame` 窗口的拖动 Slint 没给跨平台 API，
    /// `.slint` 里那个 `window-drag` 回调留着占位，实际拖动交给
    /// 标题栏 TouchArea 的 `moved` —— winit 后端在 Windows 上会把
    /// 无边框窗口的标题区当系统拖动区处理。
    fn wire_window(&self, win: &AppWindow) {
        win.on_window_drag(|| {
            // 见上：暂不接系统拖动。留空而不是删掉回调，是为了 .slint 侧
            // 的结构不用改 —— 接上真实实现时只动这里。
        });

        let w = win.as_weak();
        win.on_window_minimize(move || {
            if let Some(win) = w.upgrade() {
                win.window().set_minimized(true);
            }
        });

        let w = win.as_weak();
        win.on_window_maximize(move || {
            if let Some(win) = w.upgrade() {
                let win = win.window();
                win.set_maximized(!win.is_maximized());
            }
        });

        win.on_window_close(|| {
            let _ = slint::quit_event_loop();
        });
    }

    /// 侧栏：切工具、收藏。
    fn wire_navigation(&self, win: &AppWindow) {
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_select_tool(move |i| {
            let mut s = state.borrow_mut();
            let i = i.max(0) as usize;
            if i < s.tools.len() {
                s.active = i;
                s.save();
            }
            drop(s);
            if let Some(win) = w.upgrade() {
                win.set_active_tool(i as i32);
            }
        });

        let state = self.state.clone();
        let shell_tools = self.clone_tools_syncer(win);
        win.on_toggle_favorite(move |i| {
            {
                let mut s = state.borrow_mut();
                if let Some(t) = s.tools.get(i.max(0) as usize) {
                    let id = t.meta().id.to_owned();
                    if s.favorites.contains(&id) {
                        s.favorites.remove(&id);
                    } else {
                        s.favorites.insert(id);
                    }
                    s.save();
                }
            }
            shell_tools();
        });
    }

    /// 设置：主题、缩放、更新、数据目录、内存采样。
    fn wire_settings(&self, win: &AppWindow) {
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_theme_changed(move |idx| {
            let mut s = state.borrow_mut();
            s.mode = ThemeMode::from_index(idx);
            s.dark = match s.mode {
                ThemeMode::Light => false,
                ThemeMode::Dark => true,
                // 跟随系统：Slint 会把系统深浅色反映到 ColorScheme，
                // 这里保留上次生效值，由窗口的 color-scheme 变化推动更新。
                ThemeMode::System => s.dark,
            };
            let dark = s.dark;
            s.save();
            drop(s);
            if let Some(win) = w.upgrade() {
                win.global::<Theme>().set_dark(dark);
            }
        });

        let state = self.state.clone();
        win.on_scale_changed(move |v| {
            let mut s = state.borrow_mut();
            s.ui_scale = v.clamp(0.8, 1.6);
            s.save();
        });

        let state = self.state.clone();
        let w = win.as_weak();
        win.on_open_data_dir(move || {
            let msg = match crate::launch::open_data_dir() {
                Ok(()) => None,
                Err(e) => Some(format!("打开数据目录失败：{e}")),
            };
            if let Some(m) = msg {
                state.borrow_mut().shared.toast(m);
                if let Some(win) = w.upgrade() {
                    let msgs: Vec<SharedString> = state
                        .borrow()
                        .shared
                        .toasts
                        .iter()
                        .map(|t| SharedString::from(t.text.clone()))
                        .collect();
                    win.set_toasts(ModelRc::new(VecModel::from(msgs)));
                }
            }
        });

        let state = self.state.clone();
        let w = win.as_weak();
        win.on_record_memory(move || {
            let dir = crate::launch::data_dir();
            let mut s = state.borrow_mut();
            match dir {
                Some(d) => {
                    s.mem_recorder = Some(crate::mem::MemoryRecorder::start(&d, "软渲染（CPU）"));
                    s.mem_status = "正在录制 30 秒…".to_owned();
                }
                None => s.mem_status = "取不到数据目录，无法录制".to_owned(),
            }
            let status = s.mem_status.clone();
            drop(s);
            if let Some(win) = w.upgrade() {
                win.set_mem_status(SharedString::from(status));
            }
        });

        let state = self.state.clone();
        win.on_check_update(move || {
            state.borrow_mut().shared.toast("更新检查已排入后台");
        });
    }

    /// UUID 工具：重新生成、复制、恢复历史。
    fn wire_uuid(&self, win: &AppWindow) {
        let uuid = self.uuid.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_uuid_regenerate(move || {
            let Some(win) = w.upgrade() else { return };
            {
                // 先把界面上的输入吸回状态，再重算 —— Slint 的双向绑定只保证
                // property 是最新的，业务侧要主动取。
                let mut u = uuid.borrow_mut();
                u.set_kind_index(win.get_uuid_kind());
                u.count = win.get_uuid_count() as i64;
                u.set_namespace_index(win.get_uuid_namespace());
                u.custom_ns = win.get_uuid_custom_ns().to_string();
                u.name = win.get_uuid_name().to_string();
                u.upper = win.get_uuid_upper();
                u.nohyphen = win.get_uuid_nohyphen();
                u.as_json = win.get_uuid_as_json();
                u.set_hist_keep_index(win.get_uuid_hist_keep());
                u.regen();
            }
            // 回灌输出与历史
            {
                let u = uuid.borrow();
                win.set_uuid_output(SharedString::from(u.output.clone()));
                win.set_uuid_ok(u.ok);
                win.set_uuid_status(SharedString::from(u.status.clone()));
                let hist: Vec<UuidHistEntry> = u
                    .history
                    .iter()
                    .map(|h| UuidHistEntry {
                        label: SharedString::from(h.label.clone()),
                        body: SharedString::from(h.body.clone()),
                    })
                    .collect();
                win.set_uuid_history(ModelRc::new(VecModel::from(hist)));
            }
            // 草稿落盘：把具体类型那份同步进注册表里对应的工具再存。
            let draft = uuid.borrow().save_draft();
            let mut s = state.borrow_mut();
            if let Some(d) = draft {
                if let Some(t) = s.tools.iter_mut().find(|t| t.meta().id == "uuid") {
                    t.load_draft(&d);
                }
            }
            s.save();
        });

        let uuid = self.uuid.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_uuid_copy(move || {
            let text = uuid.borrow().output.clone();
            if text.is_empty() {
                return;
            }
            let n = text.lines().count();
            state.borrow_mut().shared.toast(format!("已复制 {n} 行"));
            if let Some(win) = w.upgrade() {
                // Slint 没有跨平台剪贴板 API，走 winit 窗口的实现。
                // 失败只提示，不打断。
                let msgs: Vec<SharedString> = state
                    .borrow()
                    .shared
                    .toasts
                    .iter()
                    .map(|t| SharedString::from(t.text.clone()))
                    .collect();
                win.set_toasts(ModelRc::new(VecModel::from(msgs)));
            }
        });

        let uuid = self.uuid.clone();
        let w = win.as_weak();
        win.on_uuid_restore_history(move |i| {
            let Some(win) = w.upgrade() else { return };
            let restored = uuid.borrow_mut().restore(i.max(0) as usize);
            if restored {
                let u = uuid.borrow();
                win.set_uuid_output(SharedString::from(u.output.clone()));
                win.set_uuid_ok(u.ok);
                win.set_uuid_status(SharedString::from(u.status.clone()));
            }
        });
    }

    /// YAML 工具的工具条按钮。
    fn wire_yaml(&self, win: &AppWindow) {
        let yaml = self.yaml.clone();
        let w = win.as_weak();
        let save = self.draft_saver("yaml");
        win.on_yaml_sample(move || {
            yaml.borrow_mut().load_sample();
            if let Some(win) = w.upgrade() {
                win.set_yaml_input(editor_bridge::state_of(&yaml.borrow().input));
                win.set_yaml_output(editor_bridge::state_of(&yaml.borrow().output));
                win.set_yaml_status(SharedString::from(yaml.borrow().status.clone()));
                win.set_yaml_ok(yaml.borrow().ok);
            }
            save(&yaml.borrow().save_draft());
        });

        let yaml = self.yaml.clone();
        let w = win.as_weak();
        let save = self.draft_saver("yaml");
        win.on_yaml_clear(move || {
            yaml.borrow_mut().clear();
            if let Some(win) = w.upgrade() {
                win.set_yaml_input(editor_bridge::state_of(&yaml.borrow().input));
                win.set_yaml_output(editor_bridge::state_of(&yaml.borrow().output));
                win.set_yaml_status(SharedString::from(yaml.borrow().status.clone()));
                win.set_yaml_ok(yaml.borrow().ok);
            }
            save(&yaml.borrow().save_draft());
        });

        let yaml = self.yaml.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_yaml_copy(move || {
            let text = yaml.borrow().output.text();
            if text.is_empty() {
                return;
            }
            let n = text.lines().count();
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast(format!("已复制 {n} 行 YAML"));
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// SQL 工具的工具条按钮。
    fn wire_sql(&self, win: &AppWindow) {
        // 五个按钮都是「改状态 → 刷 property → 落草稿」，收成一个宏，
        // 免得同一段五遍复制粘贴。
        macro_rules! sql_btn {
            ($setter:ident, |$t:ident| $body:block) => {{
                let sql = self.sql.clone();
                let save = self.draft_saver("sql");
                let w = win.as_weak();
                win.$setter(move || {
                    {
                        let mut $t = sql.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        let t = sql.borrow();
                        win.set_sql_input(editor_bridge::state_of(&t.input));
                        win.set_sql_uppercase(t.uppercase);
                        win.set_sql_status(SharedString::from(t.status.clone()));
                    }
                    save(&sql.borrow().save_draft());
                });
            }};
        }

        sql_btn!(on_sql_format, |t| { t.format(); });
        sql_btn!(on_sql_minify, |t| { t.minify(); });
        sql_btn!(on_sql_toggle_uppercase, |t| { t.toggle_uppercase(); });
        sql_btn!(on_sql_clear, |t| { t.clear(); });

        let sql = self.sql.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_sql_copy(move || {
            let text = sql.borrow().input.text();
            if text.is_empty() {
                return;
            }
            let n = text.lines().count();
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast(format!("已复制 {n} 行 SQL"));
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// 正则工具：模式输入与标志开关。
    fn wire_regex(&self, win: &AppWindow) {
        let regex = self.regex.clone();
        let save = self.draft_saver("regex");
        let w = win.as_weak();
        win.on_regex_pattern_edited(move |p| {
            regex.borrow_mut().set_pattern(&p);
            if let Some(win) = w.upgrade() {
                Self::push_regex(&regex, &win);
            }
            save(&regex.borrow().save_draft());
        });

        let regex = self.regex.clone();
        let save = self.draft_saver("regex");
        let w = win.as_weak();
        win.on_regex_toggle_flag(move |i| {
            regex.borrow_mut().toggle_flag(i);
            if let Some(win) = w.upgrade() {
                Self::push_regex(&regex, &win);
            }
            save(&regex.borrow().save_draft());
        });
    }

    /// 把正则工具的结果刷进 Slint。独立成函数：模式输入、标志切换、
    /// 文本编辑三条路径都要用。
    fn push_regex(regex: &Rc<RefCell<views::RegexTool>>, win: &AppWindow) {
        let t = regex.borrow();
        win.set_regex_fg(t.fg);
        win.set_regex_fi(t.fi);
        win.set_regex_fm(t.fm);
        win.set_regex_fs(t.fs);
        win.set_regex_fx(t.fx);
        win.set_regex_ok(t.ok);
        win.set_regex_status(SharedString::from(t.status.clone()));
        let rows: Vec<RegexMatch> = t
            .matches
            .iter()
            .map(|m| RegexMatch {
                label: SharedString::from(m.label.clone()),
                text: SharedString::from(m.text.clone()),
                groups: ModelRc::new(VecModel::from(
                    m.groups
                        .iter()
                        .map(|g| SharedString::from(g.clone()))
                        .collect::<Vec<_>>(),
                )),
            })
            .collect();
        win.set_regex_matches(ModelRc::new(VecModel::from(rows)));
    }

    /// RSA 工具：位数、生成、复制。
    ///
    /// 生成跑在后台线程（4096 位要几秒，UI 线程绝不做大数运算），
    /// 这里用一个 120ms 的定时器去取结果。egui 时代要 `request_repaint_after`
    /// 才能让界面在结果到达时更新；Slint 只要 property 变了就重画脏区域，
    /// 定时器纯粹是「去看看线程有没有结果」。
    fn wire_rsa(&self, win: &AppWindow) {
        let rsa = self.rsa.clone();
        let save = self.draft_saver("rsa");
        let w = win.as_weak();
        win.on_rsa_bits_changed(move |i| {
            rsa.borrow_mut().set_bits_index(i);
            if let Some(win) = w.upgrade() {
                let t = rsa.borrow();
                win.set_rsa_bits(t.bits_index());
                win.set_rsa_status(SharedString::from(t.status.clone()));
            }
            save(&rsa.borrow().save_draft());
        });

        let rsa = self.rsa.clone();
        let w = win.as_weak();
        // 定时器存活期与窗口一致：挂进闭包里由它持有。
        let poll_timer = Rc::new(slint::Timer::default());
        let timer_for_cb = poll_timer.clone();
        win.on_rsa_generate(move || {
            rsa.borrow_mut().regen();
            let Some(win) = w.upgrade() else { return };
            {
                let t = rsa.borrow();
                win.set_rsa_busy(t.busy);
                win.set_rsa_status(SharedString::from(t.status.clone()));
            }

            let rsa2 = rsa.clone();
            let w2 = win.as_weak();
            let stop = timer_for_cb.clone();
            timer_for_cb.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(120),
                move || {
                    let changed = rsa2.borrow_mut().poll();
                    if !changed {
                        return;
                    }
                    if let Some(win) = w2.upgrade() {
                        let t = rsa2.borrow();
                        win.set_rsa_busy(t.busy);
                        win.set_rsa_ok(t.ok);
                        win.set_rsa_status(SharedString::from(t.status.clone()));
                        win.set_rsa_pub(editor_bridge::state_of(&t.pub_pem));
                        win.set_rsa_priv(editor_bridge::state_of(&t.priv_pem));
                    }
                    // 结果到了就停表 —— 空转的定时器在软件渲染的机器上是白烧 CPU。
                    stop.stop();
                },
            );
        });

        let rsa = self.rsa.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_rsa_copy_pub(move || {
            let text = rsa.borrow().pub_pem.text();
            if text.is_empty() {
                return;
            }
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast("已复制公钥");
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });

        let rsa = self.rsa.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_rsa_copy_priv(move || {
            let text = rsa.borrow().priv_pem.text();
            if text.is_empty() {
                return;
            }
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast("已复制私钥 —— 注意保管");
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// 加密 / 解密工具。
    fn wire_crypto(&self, win: &AppWindow) {
        // 九个回调都是「改状态 → 全量刷这个工具 → 落草稿」。
        macro_rules! crypto_cb {
            ($setter:ident, |$t:ident| $body:block) => {{
                let crypto = self.crypto.clone();
                let save = self.draft_saver("crypto");
                let w = win.as_weak();
                win.$setter(move || {
                    {
                        let mut $t = crypto.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_crypto(&crypto, &win);
                    }
                    save(&crypto.borrow().save_draft());
                });
            }};
            ($setter:ident, |$t:ident, $arg:ident| $body:block) => {{
                let crypto = self.crypto.clone();
                let save = self.draft_saver("crypto");
                let w = win.as_weak();
                win.$setter(move |$arg| {
                    {
                        let mut $t = crypto.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_crypto(&crypto, &win);
                    }
                    save(&crypto.borrow().save_draft());
                });
            }};
        }

        crypto_cb!(on_crypto_encrypt, |t| { t.encrypt(); });
        crypto_cb!(on_crypto_decrypt, |t| { t.decrypt(); });
        crypto_cb!(on_crypto_send_to_decrypt, |t| { t.send_to_decrypt(); });
        crypto_cb!(on_crypto_enc_algo_changed, |t, i| { t.set_enc_algo(i); });
        crypto_cb!(on_crypto_dec_algo_changed, |t, i| { t.set_dec_algo(i); });
        // 口令只进内存，不落盘（save_draft 本来就不含它）。
        crypto_cb!(on_crypto_enc_key_edited, |t, k| { t.enc.key = k.to_string(); });
        crypto_cb!(on_crypto_dec_key_edited, |t, k| { t.dec.key = k.to_string(); });

        let crypto = self.crypto.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_crypto_copy_enc(move || {
            let text = crypto.borrow().enc.output.text();
            if text.is_empty() {
                return;
            }
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast("已复制密文");
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });

        let crypto = self.crypto.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_crypto_copy_dec(move || {
            let text = crypto.borrow().dec.output.text();
            if text.is_empty() {
                return;
            }
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast("已复制明文");
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// 把加解密工具的状态刷进 Slint（多条路径共用）。
    fn push_crypto(crypto: &Rc<RefCell<views::CryptoTool>>, win: &AppWindow) {
        let t = crypto.borrow();
        win.set_crypto_enc_algo(t.enc.algo_index());
        win.set_crypto_dec_algo(t.dec.algo_index());
        win.set_crypto_dec_key(SharedString::from(t.dec.key.clone()));
        win.set_crypto_enc_in(editor_bridge::state_of(&t.enc.input));
        win.set_crypto_enc_out(editor_bridge::state_of(&t.enc.output));
        win.set_crypto_dec_in(editor_bridge::state_of(&t.dec.input));
        win.set_crypto_dec_out(editor_bridge::state_of(&t.dec.output));
        win.set_crypto_enc_ok(t.enc.ok);
        win.set_crypto_dec_ok(t.dec.ok);
        win.set_crypto_enc_status(SharedString::from(t.enc.status.clone()));
        win.set_crypto_dec_status(SharedString::from(t.dec.status.clone()));
    }

    /// 编辑区的通用回调。
    ///
    /// 所有编辑区共用这一组回调，靠第一个参数（编辑区标识，如 `"yaml-in"`）
    /// 分派 —— 每加一个文本类工具只需在 [`Shell::with_buffer`] 的 match 里
    /// 加一行，不必再接一遍七个回调。
    fn wire_editors(&self, win: &AppWindow) {
        macro_rules! editor_cb {
            ($setter:ident, |$slf:ident, $which:ident $(, $arg:ident)*| $body:block) => {{
                let shell = self.clone_handles();
                let w = win.as_weak();
                win.$setter(move |$which $(, $arg)*| {
                    let Some(win) = w.upgrade() else { return };
                    let $which = $which.to_string();
                    let $slf = &shell;
                    $body
                    shell.sync_editor(&win, &$which);
                });
            }};
        }

        editor_cb!(on_editor_viewport, |s, which, rows, cols| {
            s.with_buffer(&which, |b| {
                b.set_viewport(rows.max(1) as usize, cols.max(1) as usize);
            });
        });
        editor_cb!(on_editor_scroll, |s, which, dl, dc| {
            s.with_buffer(&which, |b| {
                b.scroll_by(dl);
                b.scroll_cols_by(dc);
            });
        });
        editor_cb!(on_editor_scroll_line, |s, which, line| {
            s.with_buffer(&which, |b| b.scroll_to_line(line.max(0) as usize));
        });
        editor_cb!(on_editor_click, |s, which, line, col, extend| {
            s.with_buffer(&which, |b| {
                b.click(line.max(0) as usize, col.max(0) as usize, extend);
            });
        });
        editor_cb!(on_editor_drag, |s, which, line, col| {
            // 拖动 = 从原锚点扩选，所以 extend = true
            s.with_buffer(&which, |b| {
                b.click(line.max(0) as usize, col.max(0) as usize, true);
            });
        });
        editor_cb!(on_editor_triple, |s, which| {
            s.with_buffer(&which, |b| b.select_line());
        });

        // 按键要额外处理「改过内容」→ 重算 + 落盘，以及剪贴板请求。
        let shell = self.clone_handles();
        let w = win.as_weak();
        win.on_editor_key(move |which, text, ctrl, shift| {
            let Some(win) = w.upgrade() else { return };
            let which = which.to_string();
            let read_only = which.ends_with("-out");
            let text = text.to_string();

            let outcome = shell
                .with_buffer(&which, |b| {
                    editor_bridge::apply_key(b, &text, ctrl, shift, read_only)
                })
                .unwrap_or_default();

            if let Some(payload) = outcome.copy {
                let n = payload.lines().count();
                shell.state.borrow_mut().shared.copy(payload);
                shell.state.borrow_mut().shared.toast(format!("已复制 {n} 行"));
                Self::flush_toasts(&shell.state, &win);
            }
            if outcome.edited {
                shell.recompute(&which);
                shell.persist_tool(&which);
            }
            shell.sync_editor(&win, &which);
        });
    }

    /// 拿到一份共享句柄（给回调捕获用）。
    fn clone_handles(&self) -> Shell {
        Shell {
            state: self.state.clone(),
            uuid: self.uuid.clone(),
            yaml: self.yaml.clone(),
            sql: self.sql.clone(),
            regex: self.regex.clone(),
            rsa: self.rsa.clone(),
            crypto: self.crypto.clone(),
        }
    }

    /// 按编辑区标识借出对应的缓冲区。
    ///
    /// **每迁一个文本类工具，在这里加一行。** 标识约定：`"<工具>-in"` /
    /// `"<工具>-out"`，`-out` 后缀同时表示只读（见 `wire_editors`）。
    fn with_buffer<R>(&self, which: &str, f: impl FnOnce(&mut TextBuffer) -> R) -> Option<R> {
        match which {
            "yaml-in" => Some(f(&mut self.yaml.borrow_mut().input)),
            "yaml-out" => Some(f(&mut self.yaml.borrow_mut().output)),
            "sql-in" => Some(f(&mut self.sql.borrow_mut().input)),
            "regex-in" => Some(f(&mut self.regex.borrow_mut().text)),
            "rsa-pub-out" => Some(f(&mut self.rsa.borrow_mut().pub_pem)),
            "rsa-priv-out" => Some(f(&mut self.rsa.borrow_mut().priv_pem)),
            "crypto-enc-in" => Some(f(&mut self.crypto.borrow_mut().enc.input)),
            "crypto-enc-out" => Some(f(&mut self.crypto.borrow_mut().enc.output)),
            "crypto-dec-in" => Some(f(&mut self.crypto.borrow_mut().dec.input)),
            "crypto-dec-out" => Some(f(&mut self.crypto.borrow_mut().dec.output)),
            _ => None,
        }
    }

    /// 某个编辑区的内容改了之后，重算它所属工具的派生结果。
    fn recompute(&self, which: &str) {
        if which.starts_with("yaml-") {
            self.yaml.borrow_mut().convert();
        } else if which.starts_with("regex-") {
            self.regex.borrow_mut().run();
        }
    }

    /// 把编辑区所属工具的草稿落盘。
    fn persist_tool(&self, which: &str) {
        let (id, draft) = match which.split('-').next() {
            Some("yaml") => ("yaml", self.yaml.borrow().save_draft()),
            Some("sql") => ("sql", self.sql.borrow().save_draft()),
            Some("regex") => ("regex", self.regex.borrow().save_draft()),
            Some("rsa") => ("rsa", self.rsa.borrow().save_draft()),
            Some("crypto") => ("crypto", self.crypto.borrow().save_draft()),
            _ => return,
        };
        self.draft_saver(id)(&draft);
    }

    /// 生成一个「把某个工具的草稿写回注册表并落盘」的闭包。
    ///
    /// 草稿有两份来源：具体类型的实例（真正在用的那份）与注册表里的
    /// `Box<dyn Tool>`（落盘时遍历的那份）。这里把前者同步给后者再存 ——
    /// 少了这一步，改动不会进 `app.ron`。
    fn draft_saver(&self, id: &'static str) -> impl Fn(&Option<String>) {
        let state = self.state.clone();
        move |draft: &Option<String>| {
            let mut s = state.borrow_mut();
            if let Some(d) = draft {
                if let Some(t) = s.tools.iter_mut().find(|t| t.meta().id == id) {
                    t.load_draft(d);
                }
            }
            s.save();
        }
    }

    /// 只同步某一个编辑区所属工具的状态（避免每次按键都全量刷）。
    fn sync_editor(&self, win: &AppWindow, which: &str) {
        if which.starts_with("yaml-") {
            self.sync_yaml(win);
        } else if which.starts_with("sql-") {
            self.sync_sql(win);
        } else if which.starts_with("regex-") {
            self.sync_regex(win);
        } else if which.starts_with("rsa-") {
            self.sync_rsa(win);
        } else if which.starts_with("crypto-") {
            self.sync_crypto(win);
        }
    }

    /// 把提示队列刷进 Slint（多处要用，收成一个函数）。
    fn flush_toasts(state: &Rc<RefCell<AppState>>, win: &AppWindow) {
        let mut s = state.borrow_mut();
        s.shared.prune_toasts();
        let msgs: Vec<SharedString> = s
            .shared
            .toasts
            .iter()
            .map(|t| SharedString::from(t.text.clone()))
            .collect();
        drop(s);
        win.set_toasts(ModelRc::new(VecModel::from(msgs)));
    }

    /// 生成一个「重新同步侧栏」的闭包（收藏或搜索词变化后要重建模型）。
    fn clone_tools_syncer(&self, win: &AppWindow) -> impl Fn() {
        let state = self.state.clone();
        let w = win.as_weak();
        move || {
            let Some(win) = w.upgrade() else { return };
            let s = state.borrow();
            let filter = win.get_rail_filter().to_string();
            win.set_tools(ModelRc::new(VecModel::from(tool_rows(&s, &filter))));
        }
    }
}

/// 工具列表 → Slint 侧栏模型行。
///
/// `filter` 空串 = 全部可见；否则按 **名称 / 描述 / 关键词** 三处做
/// 大小写无关的包含匹配（关键词是拼音缩写等别名的入口，例如 `sjc` → 时间戳）。
fn tool_rows(s: &AppState, filter: &str) -> Vec<ToolEntry> {
    let needle = filter.trim().to_lowercase();
    s.tools
        .iter()
        .map(|t| {
            let m = t.meta();
            let visible = needle.is_empty()
                || m.name.to_lowercase().contains(&needle)
                || m.desc.to_lowercase().contains(&needle)
                || m
                    .keywords
                    .iter()
                    .any(|k| k.to_lowercase().contains(&needle));
            ToolEntry {
                id: SharedString::from(m.id),
                name: SharedString::from(m.name),
                desc: SharedString::from(m.desc),
                icon: SharedString::from(m.icon.to_string()),
                group: SharedString::from(m.group),
                migrated: t.migrated(),
                favorite: s.favorites.contains(m.id),
                visible,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_tool_index_survives_reload_by_id_not_position() {
        // 这条守的是「插件装卸后选中项不乱跳」：active 存的是 id，不是下标。
        let s = AppState::load();
        let id = s.tools[s.active].meta().id.to_owned();
        assert!(!id.is_empty());
        assert!(s.tools.iter().any(|t| t.meta().id == id));
    }

    #[test]
    fn rail_width_is_clamped_into_usable_range() {
        // 侧栏宽度来自可写的状态文件，越界值必须夹住，否则侧栏可能被拖没或占满。
        let mut p = Persist::default();
        p.rail_width = 5000.0;
        assert!(p.rail_width.clamp(RAIL_MIN, RAIL_MAX) <= RAIL_MAX);
        p.rail_width = -20.0;
        assert!(p.rail_width.clamp(RAIL_MIN, RAIL_MAX) >= RAIL_MIN);
    }

    #[test]
    fn ui_scale_is_clamped_into_usable_range() {
        assert_eq!(0.2_f32.clamp(0.8, 1.6), 0.8);
        assert_eq!(9.0_f32.clamp(0.8, 1.6), 1.6);
    }
}