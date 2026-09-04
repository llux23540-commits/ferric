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
    /// 更新器（检查 / 下载 / 校验，全在后台线程）。
    pub updater: crate::updater::Updater,
    /// 更新框是否打开（点「稍后」后关闭，但顶栏入口还在）。
    pub update_dialog_open: bool,
    /// 更新器启动至今的秒数 —— `Updater::tick` 用它做「启动后延迟首检」。
    update_clock: f64,
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
            updater: crate::updater::Updater::default(),
            update_dialog_open: false,
            update_clock: 0.0,
            mem_recorder: None,
            mem_status: String::new(),
        }
    }

    /// 距上次成功检查是否已经够久（跨启动节流）。
    ///
    /// 只放内存里的话，开十次应用就查十次 —— 检查会把本机版本号发给服务器。
    pub fn update_check_is_stale(&self) -> bool {
        match self.last_update_check {
            None => true,
            Some(t) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                now.saturating_sub(t) >= crate::updater::AUTO_CHECK_INTERVAL_SECS as i64
            }
        }
    }

    /// 当前选中项若是 WASM 插件，借出它。
    ///
    /// 插件在 `tools` 里内置工具之后那一截（`builtin_tools..`），装卸时整段
    /// 重建，所以不像内置工具那样有独立字段 —— 按当前选中项现取。
    pub fn active_plugin(&self) -> Option<&crate::plugin_host::PluginTool> {
        if self.active < self.builtin_tools {
            return None;
        }
        self.tools.get(self.active)?.as_plugin()
    }

    pub fn active_plugin_mut(&mut self) -> Option<&mut crate::plugin_host::PluginTool> {
        if self.active < self.builtin_tools {
            return None;
        }
        let idx = self.active;
        self.tools.get_mut(idx)?.as_plugin_mut()
    }

    /// 当前生效的数据源（自建服务端 / GitHub 发布页 / 演示数据）。
    pub fn source(&self) -> Option<crate::source::Source> {
        crate::source::Source::resolve(
            self.source_pref,
            self.server_override.clone(),
            self.github_override
                .clone()
                .or_else(crate::github::GithubSource::builtin),
        )
    }

    /// 热加载插件：把插件那截工具换成磁盘上的当前状态，内置工具原封不动。
    ///
    /// 保留当前选中的工具（按 id 找回；插件被卸载则回落到市场页）与各插件的
    /// 输入草稿。装 / 卸插件之后不必重启 —— 这条是 egui 版就有的承诺。
    ///
    /// ⚠️ `PluginTool` 的 `ToolMeta` 需要 `&'static str`，加载时用 `Box::leak`
    /// 得到，因此每次热加载会漏掉一份插件元数据（每个插件几百字节）。
    /// 装插件是低频动作，这点代价换「不用重启」是划算的；但别把这个函数
    /// 接到什么每帧调用的地方去。
    pub fn reload_plugins(&mut self) {
        let active_id = self
            .tools
            .get(self.active)
            .map(|t| t.meta().id.to_owned())
            .unwrap_or_default();
        // 插件的输入草稿：重建前先收起来，重建后按 id 放回去。
        let drafts: std::collections::HashMap<String, String> = self.tools[self.builtin_tools..]
            .iter()
            .filter_map(|t| t.save_draft().map(|d| (t.meta().id.to_owned(), d)))
            .collect();

        self.tools.truncate(self.builtin_tools);
        let (plugin_tools, warns) = crate::plugin_host::load_all();
        for mut t in plugin_tools {
            if let Some(d) = drafts.get(t.meta().id) {
                t.load_draft(d);
            }
            self.tools.push(Box::new(t));
        }
        for w in warns {
            self.shared.toast(format!("插件加载失败 · {w}"));
        }

        // 选中的工具可能刚被卸载 —— 找不回来就退回插件市场（用户就是从那儿来的）。
        self.active = self
            .tools
            .iter()
            .position(|t| t.meta().id == active_id)
            .or_else(|| self.tools.iter().position(|t| t.meta().id == "market"))
            .unwrap_or(0);
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
}

/// 已迁移的工具各持一个具体类型的字段。
///
/// 为什么不从 `Vec<Box<dyn Tool>>` 里向下转型：那需要给 `Tool` 加一个
/// 只服务于转型的通用后门。插件是唯一例外（要读 manifest 渲染控件），
/// 用 `Tool::as_plugin` 把能力限定得刚好够用。
pub struct Shell {
    pub state: Rc<RefCell<AppState>>,
    pub uuid: Rc<RefCell<views::UuidTool>>,
    pub yaml: Rc<RefCell<views::YamlTool>>,
    pub sql: Rc<RefCell<views::SqlTool>>,
    pub regex: Rc<RefCell<views::RegexTool>>,
    pub rsa: Rc<RefCell<views::RsaTool>>,
    pub crypto: Rc<RefCell<views::CryptoTool>>,
    pub gm: Rc<RefCell<views::GmTool>>,
    pub ts: Rc<RefCell<views::TimestampTool>>,
    pub json: Rc<RefCell<views::JsonTool>>,
    pub diff: Rc<RefCell<views::DiffTool>>,
    pub market: Rc<RefCell<views::MarketTool>>,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
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
        let mut gm = views::GmTool::default();
        if let Some(d) = draft_of("gm") {
            gm.load_draft(&d);
        }
        let mut ts = views::TimestampTool::default();
        if let Some(d) = draft_of("timestamp") {
            ts.load_draft(&d);
        }
        let mut json = views::JsonTool::default();
        if let Some(d) = draft_of("json") {
            json.load_draft(&d);
        }
        let mut diff = views::DiffTool::default();
        if let Some(d) = draft_of("diff") {
            diff.load_draft(&d);
        }
        // 市场不持久化草稿（列表是服务端状态）。
        let market = views::MarketTool::default();

        Self {
            state: Rc::new(RefCell::new(state)),
            uuid: Rc::new(RefCell::new(uuid)),
            yaml: Rc::new(RefCell::new(yaml)),
            sql: Rc::new(RefCell::new(sql)),
            regex: Rc::new(RefCell::new(regex)),
            rsa: Rc::new(RefCell::new(rsa)),
            crypto: Rc::new(RefCell::new(crypto)),
            gm: Rc::new(RefCell::new(gm)),
            ts: Rc::new(RefCell::new(ts)),
            json: Rc::new(RefCell::new(json)),
            diff: Rc::new(RefCell::new(diff)),
            market: Rc::new(RefCell::new(market)),
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
        self.sync_gm(win);
        self.sync_ts(win);
        self.sync_json(win);
        self.sync_diff(win);
        self.sync_market(win);
        Self::sync_plugin(&self.state, win);
        Self::push_update(&self.state, win);
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

    /// 国密 SM 工具状态 → property。下拉的标签只在这里灌一次（它们是常量）。
    fn sync_gm(&self, win: &AppWindow) {
        let labels = |v: Vec<&'static str>| {
            ModelRc::new(VecModel::from(
                v.into_iter().map(SharedString::from).collect::<Vec<_>>(),
            ))
        };
        win.set_gm_enc_algos(labels(views::GmTool::enc_algo_labels()));
        win.set_gm_dec_algos(labels(views::GmTool::dec_algo_labels()));
        win.set_gm_fmts(labels(views::GmTool::fmt_labels()));
        Self::push_gm(&self.gm, win);
    }

    /// 把国密工具的状态刷进 Slint（多条路径共用）。
    fn push_gm(gm: &Rc<RefCell<views::GmTool>>, win: &AppWindow) {
        let t = gm.borrow();
        win.set_gm_pub_key(SharedString::from(t.pub_key.clone()));
        win.set_gm_priv_key(SharedString::from(t.priv_key.clone()));
        win.set_gm_key_status(SharedString::from(t.key_status.clone()));
        win.set_gm_enc_algo(t.enc_algo_index());
        win.set_gm_dec_algo(t.dec_algo_index());
        win.set_gm_fmt(t.fmt_index());
        win.set_gm_enc_needs_key(t.enc_needs_key());
        // SM2 那一栏放的是公钥 / 私钥，其余算法放的是口令 —— 标签要跟着变，
        // 否则用户会把口令填进本该放公钥的框里，然后收到一条看不懂的报错。
        win.set_gm_enc_key_label(SharedString::from(if t.enc_key_is_pubkey() {
            "公钥（SM2）"
        } else {
            "口令"
        }));
        win.set_gm_dec_key_label(SharedString::from(if t.dec_key_is_privkey() {
            "私钥（SM2）"
        } else {
            "口令"
        }));
        win.set_gm_enc_key(SharedString::from(t.enc_key.clone()));
        win.set_gm_dec_key(SharedString::from(t.dec_key.clone()));
        win.set_gm_enc_in(editor_bridge::state_of(&t.enc_input));
        win.set_gm_enc_out(editor_bridge::state_of(&t.enc_output));
        win.set_gm_dec_in(editor_bridge::state_of(&t.dec_input));
        win.set_gm_dec_out(editor_bridge::state_of(&t.dec_output));
        win.set_gm_enc_ok(t.enc_ok);
        win.set_gm_dec_ok(t.dec_ok);
        win.set_gm_enc_status(SharedString::from(t.enc_status.clone()));
        win.set_gm_dec_status(SharedString::from(t.dec_status.clone()));
        win.set_gm_sig(editor_bridge::state_of(&t.sig_text));
        win.set_gm_sig_hex(SharedString::from(t.sig_hex.clone()));
        win.set_gm_sig_ok(t.sig_ok);
        win.set_gm_sig_status(SharedString::from(t.sig_status.clone()));
    }

    /// 时间戳工具状态 → property。
    fn sync_ts(&self, win: &AppWindow) {
        Self::push_ts(&self.ts, win, true);
    }

    /// 把时间戳工具刷进 Slint。
    ///
    /// `with_list` = 是否重建时区列表。列表有 597 条，实时刷新每秒都重建
    /// 一次纯属白烧 —— 只在筛选词变化或首次同步时给 true。
    fn push_ts(ts: &Rc<RefCell<views::TimestampTool>>, win: &AppWindow, with_list: bool) {
        let t = ts.borrow();
        win.set_ts_now(SharedString::from(t.now_seconds().to_string()));
        win.set_ts_now_ms(SharedString::from(format!("{} 毫秒", t.now_ms)));
        win.set_ts_running(t.running);
        win.set_ts_offset(SharedString::from(t.offset.clone()));
        win.set_ts_tz_label(SharedString::from(t.tz_label_current()));
        win.set_ts_ts_input(SharedString::from(t.ts_input.clone()));
        win.set_ts_ts_output(SharedString::from(t.ts_output.clone()));
        win.set_ts_ts_ok(t.ts_ok);
        win.set_ts_date_input(SharedString::from(t.date_input.clone()));
        win.set_ts_date_output(SharedString::from(t.date_output.clone()));
        win.set_ts_date_ok(t.date_ok);
        if with_list {
            let rows: Vec<TzRow> = t
                .tz_hits
                .iter()
                .map(|r| TzRow {
                    name: SharedString::from(r.name.clone()),
                    label: SharedString::from(r.label.clone()),
                })
                .collect();
            win.set_ts_tz_hits(ModelRc::new(VecModel::from(rows)));
        }
    }

    /// JSON 工具状态 → property。
    fn sync_json(&self, win: &AppWindow) {
        let t = self.json.borrow();
        win.set_json_input(editor_bridge::state_of(&t.input));
        win.set_json_indent(t.indent_index());
        win.set_json_sort(t.sort);
        win.set_json_wrap(t.wrap);
        win.set_json_ok(t.ok);
        win.set_json_status(SharedString::from(t.status.clone()));
        win.set_json_find(SharedString::from(t.find.clone()));
        win.set_json_hit_count(t.hits.len() as i32);
        win.set_json_hit_index(t.hit_idx as i32);
        win.set_json_can_undo(t.can_undo());
        win.set_json_can_redo(t.can_redo());
    }

    /// 对比工具状态 → property。
    fn sync_diff(&self, win: &AppWindow) {
        Self::push_diff(&self.diff, win);
    }

    /// 把对比结果刷进 Slint。
    fn push_diff(diff: &Rc<RefCell<views::DiffTool>>, win: &AppWindow) {
        use ferric_core::diff::Tag;
        let t = diff.borrow();
        win.set_diff_left(editor_bridge::state_of(&t.left));
        win.set_diff_right(editor_bridge::state_of(&t.right));
        win.set_diff_only_changes(t.only_changes);
        win.set_diff_status(SharedString::from(t.status.clone()));

        let rows: Vec<DiffRow> = t
            .rows
            .iter()
            .map(|r| DiffRow {
                sign: SharedString::from(r.sign),
                left_no: SharedString::from(r.left_no.clone()),
                right_no: SharedString::from(r.right_no.clone()),
                kind: match r.tag {
                    Tag::Equal => 0,
                    Tag::Delete => 1,
                    Tag::Insert => 2,
                },
                segs: ModelRc::new(VecModel::from(
                    r.segs
                        .iter()
                        .map(|(text, emph)| DiffSeg {
                            text: SharedString::from(text.clone()),
                            emph: *emph,
                        })
                        .collect::<Vec<_>>(),
                )),
            })
            .collect();
        win.set_diff_rows(ModelRc::new(VecModel::from(rows)));
    }

    /// 插件市场状态 → property。
    fn sync_market(&self, win: &AppWindow) {
        Self::push_market(&self.market, win);
    }

    /// 把市场状态刷进 Slint。
    fn push_market(market: &Rc<RefCell<views::MarketTool>>, win: &AppWindow) {
        let t = market.borrow();
        win.set_market_query(SharedString::from(t.query.clone()));
        win.set_market_loading(t.loading);
        win.set_market_ok(t.ok);
        win.set_market_status(SharedString::from(t.status.clone()));
        win.set_market_progress(t.progress_pct());
        win.set_market_installing(t.installing.is_some());
        win.set_market_pending(t.pending_updates() as i32);

        let cards: Vec<PluginCard> = t
            .items
            .iter()
            .map(|i| PluginCard {
                slug: SharedString::from(i.slug.clone()),
                name: SharedString::from(i.name.clone()),
                desc: SharedString::from(i.desc.clone()),
                version: SharedString::from(i.version.clone()),
                installed: SharedString::from(i.installed.clone().unwrap_or_default()),
                has_update: i.has_update,
                is_installed: i.installed.is_some(),
                size_text: SharedString::from(fmt_size(i.size)),
                downloads_text: SharedString::from(format!("{} 次下载", i.downloads)),
                busy: t.installing.as_deref() == Some(i.slug.as_str()),
            })
            .collect();
        win.set_market_cards(ModelRc::new(VecModel::from(cards)));
    }

    /// 当前选中的 WASM 插件 → property。
    ///
    /// 插件不像内置工具那样有专属字段：它们在 `AppState::tools` 里（内置工具
    /// 之后那一截），装卸时整段重建。所以这里按当前选中项现取，
    /// 而不是持一个 `Rc<RefCell<PluginTool>>`。
    fn sync_plugin(state: &Rc<RefCell<AppState>>, win: &AppWindow) {
        let s = state.borrow();
        let Some(p) = s.active_plugin() else {
            win.set_current_is_plugin(false);
            return;
        };
        win.set_current_is_plugin(true);
        win.set_plugin_ok(p.ok);
        win.set_plugin_status(SharedString::from(p.status.clone()));
        win.set_plugin_in(editor_bridge::state_of(&p.input));
        win.set_plugin_out(editor_bridge::state_of(&p.output));

        let rows: Vec<PluginOption> = p
            .option_rows()
            .into_iter()
            .map(|r| PluginOption {
                kind: match r.kind {
                    crate::plugin_host::OptKind::Seg => 0,
                    crate::plugin_host::OptKind::Toggle => 1,
                    crate::plugin_host::OptKind::Text => 2,
                },
                label: SharedString::from(r.label),
                values: ModelRc::new(VecModel::from(
                    r.values.into_iter().map(SharedString::from).collect::<Vec<_>>(),
                )),
                selected: r.selected as i32,
                on: r.on,
                text: SharedString::from(r.text),
                hint: SharedString::from(r.hint),
            })
            .collect();
        win.set_plugin_options(ModelRc::new(VecModel::from(rows)));
    }

    /// 更新器状态 → property。
    fn push_update(state: &Rc<RefCell<AppState>>, win: &AppWindow) {
        use crate::updater::Phase;
        let s = state.borrow();
        let (phase, version, progress, note) = match &s.updater.phase {
            Phase::Idle => (0, String::new(), 0, String::new()),
            Phase::Checking => (1, String::new(), 0, String::new()),
            Phase::UpToDate => (2, String::new(), 0, String::new()),
            Phase::Available(i) => (3, i.version.clone(), 0, i.notes.clone()),
            Phase::Downloading { done, total } => {
                let pct = if *total == 0 {
                    0
                } else {
                    ((*done as f64 / *total as f64) * 100.0).clamp(0.0, 100.0) as i32
                };
                (4, String::new(), pct, String::new())
            }
            Phase::Ready { info, .. } => (5, info.version.clone(), 100, info.notes.clone()),
            // 失败**必须与「已最新」分开显示** —— 否则中间人丢包就能伪装成已最新。
            Phase::Failed(e) => (6, String::new(), 0, e.clone()),
        };
        win.set_update_phase(phase);
        win.set_update_version(SharedString::from(version));
        win.set_update_progress(progress);
        win.set_update_note(SharedString::from(note));
        win.set_update_dialog_open(s.update_dialog_open);
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
        self.wire_gm(win);
        self.wire_ts(win);
        self.wire_json(win);
        self.wire_diff(win);
        self.wire_market(win);
        self.wire_plugin(win);
        self.wire_updater(win);
        self.wire_editors(win);
    }

    /// 窗口按钮：拖动 / 最小化 / 最大化 / 关闭。
    ///
    /// 自绘标题栏（`no-frame`）意味着这四件事都得自己接。
    fn wire_window(&self, win: &AppWindow) {
        let w = win.as_weak();
        win.on_window_drag(move || {
            let Some(win) = w.upgrade() else { return };
            // 交给系统做拖动（winit 的 drag_window）。自己按鼠标位移改窗口
            // 坐标也能动，但那样拖动期间每帧都要重排 + 重画整窗 ——
            // 软件渲染的机器上会明显拖影。系统拖动是合成器负责搬运。
            //
            // 走 unstable-winit-030：Slint 没有跨平台的「开始拖动窗口」API，
            // 而无边框窗口（no-frame）没有系统标题栏可拖。失败静默 ——
            // 拖不动窗口不该变成一条报错。
            use slint::winit_030::WinitWindowAccessor;
            win.window().with_winit_window(|ww| {
                let _ = ww.drag_window();
            });
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
        let market = self.market.clone();
        let w = win.as_weak();
        win.on_select_tool(move |i| {
            let i = i.max(0) as usize;
            let picked = {
                let mut s = state.borrow_mut();
                if i >= s.tools.len() {
                    return;
                }
                s.active = i;
                s.save();
                s.tools[i].meta().id
            };
            let Some(win) = w.upgrade() else { return };
            win.set_active_tool(i as i32);

            // 切到插件就把它的 manifest 控件与输入输出灌进去
            //（插件没有独立字段，是按当前选中项现取的）。
            Self::sync_plugin(&state, &win);

            // 进插件市场就自动拉一次列表。以前必须先点「刷新」才有内容 ——
            // 用户点开「插件市场」本来就是为了看列表。
            if picked == "market" {
                let src = state.borrow().source();
                market.borrow_mut().on_enter(src.as_ref());
                Self::push_market(&market, &win);
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

        // 「重置演示数据」：把演示源的已装插件记录清掉。
        // 只在演示数据源下有意义 —— 它碰不到任何安全边界（那条路只接受
        // 验签通过的字节），能造成的最坏结果就是界面上少几条假数据。
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_reset_demo(move || {
            let src = state.borrow().source();
            match src {
                Some(src) => {
                    crate::market::reset_demo(&src);
                    state.borrow_mut().shared.toast("已重置演示数据");
                }
                None => state.borrow_mut().shared.toast("当前数据源没有演示数据"),
            }
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });

        let state = self.state.clone();
        let w = win.as_weak();
        win.on_check_update(move || {
            let src = state.borrow().source();
            match src {
                Some(src) => {
                    state.borrow_mut().updater.check(src);
                    state.borrow_mut().shared.toast("正在检查更新…");
                }
                None => {
                    state
                        .borrow_mut()
                        .shared
                        .toast("本构建未配置更新源（设置 → 数据源）");
                }
            }
            if let Some(win) = w.upgrade() {
                Self::push_update(&state, &win);
                Self::flush_toasts(&state, &win);
            }
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

    /// 国密 SM 工具。
    fn wire_gm(&self, win: &AppWindow) {
        macro_rules! gm_cb {
            ($setter:ident, |$t:ident| $body:block) => {{
                let gm = self.gm.clone();
                let save = self.draft_saver("gm");
                let w = win.as_weak();
                win.$setter(move || {
                    {
                        let mut $t = gm.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_gm(&gm, &win);
                    }
                    save(&gm.borrow().save_draft());
                });
            }};
            ($setter:ident, |$t:ident, $arg:ident| $body:block) => {{
                let gm = self.gm.clone();
                let save = self.draft_saver("gm");
                let w = win.as_weak();
                win.$setter(move |$arg| {
                    {
                        let mut $t = gm.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_gm(&gm, &win);
                    }
                    save(&gm.borrow().save_draft());
                });
            }};
        }

        gm_cb!(on_gm_gen_keypair, |t| { t.gen_keypair(); });
        gm_cb!(on_gm_derive_pub, |t| { t.derive_pub(); });
        gm_cb!(on_gm_encrypt, |t| { t.encrypt(); });
        gm_cb!(on_gm_decrypt, |t| { t.decrypt(); });
        gm_cb!(on_gm_send_to_decrypt, |t| { t.send_to_decrypt(); });
        gm_cb!(on_gm_sign, |t| { t.sign(); });
        gm_cb!(on_gm_verify, |t| { t.verify(); });
        gm_cb!(on_gm_enc_algo_changed, |t, i| { t.set_enc_algo(i); });
        gm_cb!(on_gm_dec_algo_changed, |t, i| { t.set_dec_algo(i); });
        gm_cb!(on_gm_fmt_changed, |t, i| { t.set_fmt(i); });
        gm_cb!(on_gm_enc_key_edited, |t, k| { t.enc_key = k.to_string(); });
        gm_cb!(on_gm_dec_key_edited, |t, k| { t.dec_key = k.to_string(); });
        gm_cb!(on_gm_pub_key_edited, |t, k| { t.pub_key = k.to_string(); });
        gm_cb!(on_gm_priv_key_edited, |t, k| { t.priv_key = k.to_string(); });
        gm_cb!(on_gm_sig_hex_edited, |t, h| { t.sig_hex = h.to_string(); });
    }

    /// 时间戳工具。
    ///
    /// 实时刷新由一个 1 秒定时器驱动。egui 时代这里需要一整套省帧逻辑
    ///（对齐秒边界 / 失焦零调度 / 静置 90 秒停表），因为立即模式为了让秒数
    /// 跳动就得整窗重绘，而软件光栅化下整窗重绘要上百毫秒。Slint 只重画
    /// 那一小块脏区域，所以这里只留开关本身。
    fn wire_ts(&self, win: &AppWindow) {
        // 秒定时器常驻：tick() 在没开实时刷新时直接返回 false，不碰 property，
        // 因此不会引起任何重绘。
        let ts = self.ts.clone();
        let w = win.as_weak();
        let clock = Rc::new(slint::Timer::default());
        let keep = clock.clone();
        clock.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(1),
            move || {
                if !ts.borrow_mut().tick() {
                    return;
                }
                if let Some(win) = w.upgrade() {
                    Self::push_ts(&ts, &win, false);
                }
            },
        );
        // 定时器的所有权挂到窗口上：Timer drop 了就停表。
        // 用一个不会被调用的闭包持有它 —— Slint 没有「把任意对象挂到窗口」的 API。
        let ts_hold = self.ts.clone();
        win.on_ts_refresh_now(move || {
            let _keep_alive = &keep;
            ts_hold.borrow_mut().refresh_now();
        });

        macro_rules! ts_cb {
            ($setter:ident, $with_list:expr, |$t:ident| $body:block) => {{
                let ts = self.ts.clone();
                let save = self.draft_saver("timestamp");
                let w = win.as_weak();
                win.$setter(move || {
                    {
                        let mut $t = ts.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_ts(&ts, &win, $with_list);
                    }
                    save(&ts.borrow().save_draft());
                });
            }};
            ($setter:ident, $with_list:expr, |$t:ident, $arg:ident| $body:block) => {{
                let ts = self.ts.clone();
                let save = self.draft_saver("timestamp");
                let w = win.as_weak();
                win.$setter(move |$arg| {
                    {
                        let mut $t = ts.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_ts(&ts, &win, $with_list);
                    }
                    save(&ts.borrow().save_draft());
                });
            }};
        }

        ts_cb!(on_ts_toggle_running, false, |t| { t.toggle_running(); });
        ts_cb!(on_ts_use_now, false, |t| { t.use_now(); });
        ts_cb!(on_ts_input_edited, false, |t, v| { t.set_ts_input(&v); });
        ts_cb!(on_ts_date_edited, false, |t, v| { t.set_date_input(&v); });
        // 这两个会改时区列表 / 选中项，要重建列表
        ts_cb!(on_ts_filter_edited, true, |t, f| { t.set_filter(&f); });
        ts_cb!(on_ts_select_tz, false, |t, n| { t.select_tz(&n); });

        let ts = self.ts.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_ts_copy_now(move || {
            let v = ts.borrow().now_seconds().to_string();
            state.borrow_mut().shared.copy(v.clone());
            state.borrow_mut().shared.toast(format!("已复制 {v}"));
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// JSON 工具。
    fn wire_json(&self, win: &AppWindow) {
        macro_rules! json_cb {
            ($setter:ident, |$t:ident| $body:block) => {{
                let json = self.json.clone();
                let save = self.draft_saver("json");
                let w = win.as_weak();
                win.$setter(move || {
                    {
                        let mut $t = json.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_json(&json, &win);
                    }
                    save(&json.borrow().save_draft());
                });
            }};
            ($setter:ident, |$t:ident, $arg:ident| $body:block) => {{
                let json = self.json.clone();
                let save = self.draft_saver("json");
                let w = win.as_weak();
                win.$setter(move |$arg| {
                    {
                        let mut $t = json.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_json(&json, &win);
                    }
                    save(&json.borrow().save_draft());
                });
            }};
        }

        json_cb!(on_json_format, |t| { t.format(); });
        json_cb!(on_json_minify, |t| { t.minify(); });
        json_cb!(on_json_escape, |t| { t.escape(); });
        json_cb!(on_json_unescape, |t| { t.unescape(); });
        json_cb!(on_json_toggle_sort, |t| { t.toggle_sort(); });
        json_cb!(on_json_toggle_wrap, |t| { t.toggle_wrap(); });
        json_cb!(on_json_clear, |t| { t.clear(); });
        json_cb!(on_json_undo, |t| { t.undo(); });
        json_cb!(on_json_redo, |t| { t.redo(); });
        json_cb!(on_json_search, |t| { t.search(); });
        json_cb!(on_json_next_hit, |t| { t.next_hit(); });
        json_cb!(on_json_prev_hit, |t| { t.prev_hit(); });
        json_cb!(on_json_indent_changed, |t, i| { t.set_indent_index(i); });
        json_cb!(on_json_find_edited, |t, f| { t.set_find(&f); });

        let json = self.json.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_json_copy(move || {
            let text = json.borrow().input.text();
            if text.is_empty() {
                return;
            }
            let n = text.lines().count();
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast(format!("已复制 {n} 行"));
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// 把 JSON 工具的状态刷进 Slint（多条路径共用）。
    fn push_json(json: &Rc<RefCell<views::JsonTool>>, win: &AppWindow) {
        let t = json.borrow();
        win.set_json_input(editor_bridge::state_of(&t.input));
        win.set_json_indent(t.indent_index());
        win.set_json_sort(t.sort);
        win.set_json_wrap(t.wrap);
        win.set_json_ok(t.ok);
        win.set_json_status(SharedString::from(t.status.clone()));
        win.set_json_hit_count(t.hits.len() as i32);
        win.set_json_hit_index(t.hit_idx as i32);
        win.set_json_can_undo(t.can_undo());
        win.set_json_can_redo(t.can_redo());
    }

    /// 对比工具。
    fn wire_diff(&self, win: &AppWindow) {
        macro_rules! diff_cb {
            ($setter:ident, |$t:ident| $body:block) => {{
                let diff = self.diff.clone();
                let save = self.draft_saver("diff");
                let w = win.as_weak();
                win.$setter(move || {
                    {
                        let mut $t = diff.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_diff(&diff, &win);
                    }
                    save(&diff.borrow().save_draft());
                });
            }};
        }

        diff_cb!(on_diff_compare, |t| { t.compare(); });
        diff_cb!(on_diff_toggle_only_changes, |t| { t.toggle_only_changes(); });
        diff_cb!(on_diff_swap, |t| { t.swap(); });
        diff_cb!(on_diff_clear, |t| { t.clear(); });

        let diff = self.diff.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_diff_copy(move || {
            let text = diff.borrow().as_text();
            if text.trim().is_empty() {
                return;
            }
            let n = text.lines().count();
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast(format!("已复制 {n} 行差异"));
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// 插件市场。
    ///
    /// 网络与安装都在后台线程，这里用一个 200ms 定时器取结果。
    /// 装完 / 卸完会置 `changed`，外壳据此热加载插件目录 —— 装完立刻生效，
    /// 不必重启（这条是 egui 版就有的承诺，迁移后必须保住）。
    fn wire_market(&self, win: &AppWindow) {
        let market = self.market.clone();
        let state = self.state.clone();
        let w = win.as_weak();
        let poll = Rc::new(slint::Timer::default());
        let poll_for_cb = poll.clone();

        // 常驻定时器：没有后台任务时 poll() 直接返回 false，不碰 property。
        let m2 = market.clone();
        let s2 = state.clone();
        let w2 = win.as_weak();
        poll.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(200),
            move || {
                let src = s2.borrow().source();
                if !m2.borrow_mut().poll(src.as_ref()) {
                    return;
                }
                let Some(win) = w2.upgrade() else { return };
                Self::push_market(&m2, &win);
                // 装 / 卸完了 → 热加载插件目录，并把提示排出去。
                if m2.borrow_mut().take_changed() {
                    let msg = {
                        let mut st = s2.borrow_mut();
                        st.reload_plugins();
                        "插件目录已重新加载"
                    };
                    s2.borrow_mut().shared.toast(msg);
                    Self::flush_toasts(&s2, &win);
                }
            },
        );

        macro_rules! market_cb {
            ($setter:ident, |$t:ident, $src:ident| $body:block) => {{
                let market = market.clone();
                let state = state.clone();
                let w = w.clone();
                let _keep = poll_for_cb.clone();
                win.$setter(move || {
                    let _keep_alive = &_keep;
                    let $src = state.borrow().source();
                    {
                        let mut $t = market.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_market(&market, &win);
                    }
                });
            }};
            ($setter:ident, |$t:ident, $src:ident, $arg:ident| $body:block) => {{
                let market = market.clone();
                let state = state.clone();
                let w = w.clone();
                let _keep = poll_for_cb.clone();
                win.$setter(move |$arg| {
                    let _keep_alive = &_keep;
                    let $src = state.borrow().source();
                    {
                        let mut $t = market.borrow_mut();
                        $body
                    }
                    if let Some(win) = w.upgrade() {
                        Self::push_market(&market, &win);
                    }
                });
            }};
        }

        market_cb!(on_market_refresh, |t, src| { t.refresh(src.as_ref()); });
        market_cb!(on_market_update_all, |t, src| { t.update_all(src.as_ref()); });
        market_cb!(on_market_install, |t, src, slug| { t.install(src.as_ref(), &slug); });
        market_cb!(on_market_uninstall, |t, src, slug| { t.uninstall(src.as_ref(), &slug); });
        market_cb!(on_market_query_edited, |t, _src, q| { t.set_query(&q); });
    }

    /// WASM 插件视图。
    fn wire_plugin(&self, win: &AppWindow) {
        let state = self.state.clone();
        let w = win.as_weak();
        win.on_plugin_option_changed(move |idx, num, text| {
            {
                let mut s = state.borrow_mut();
                if let Some(p) = s.active_plugin_mut() {
                    p.set_option(idx.max(0) as usize, num, &text);
                    p.run_if_dirty();
                }
            }
            state.borrow().save();
            if let Some(win) = w.upgrade() {
                Self::sync_plugin(&state, &win);
            }
        });

        let state = self.state.clone();
        let w = win.as_weak();
        win.on_plugin_run(move || {
            {
                let mut s = state.borrow_mut();
                if let Some(p) = s.active_plugin_mut() {
                    p.run_now();
                }
            }
            state.borrow().save();
            if let Some(win) = w.upgrade() {
                Self::sync_plugin(&state, &win);
            }
        });

        let state = self.state.clone();
        let w = win.as_weak();
        win.on_plugin_copy(move || {
            let text = {
                let s = state.borrow();
                s.active_plugin().map(|p| p.output.text()).unwrap_or_default()
            };
            if text.is_empty() {
                return;
            }
            let n = text.lines().count();
            state.borrow_mut().shared.copy(text);
            state.borrow_mut().shared.toast(format!("已复制 {n} 行"));
            if let Some(win) = w.upgrade() {
                Self::flush_toasts(&state, &win);
            }
        });
    }

    /// 更新器。
    ///
    /// 「检查 → 下载 → 安装」里**只有最后一步需要人点**：后台绝不自动安装，
    /// 那一步会关掉用户正在用的应用。自动后台下载也只对内置服务器开放
    ///（判断在 `Source::allows_auto_download`）。
    fn wire_updater(&self, win: &AppWindow) {
        // 1 秒心跳：推进 tick（到点自动检查 / 发现新版转下载 / 就绪通知一次）
        // 并收后台线程的消息。空闲时这两个调用都不碰 property，不产生重绘。
        let state = self.state.clone();
        let w = win.as_weak();
        let beat = Rc::new(slint::Timer::default());
        let keep = beat.clone();
        beat.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(1),
            move || {
                let Some(win) = w.upgrade() else { return };
                let before = {
                    let s = state.borrow();
                    std::mem::discriminant(&s.updater.phase)
                };

                let (src, auto, stale, now) = {
                    let mut s = state.borrow_mut();
                    s.update_clock += 1.0;
                    (s.source(), s.auto_update, s.update_check_is_stale(), s.update_clock)
                };

                let tick = {
                    let mut s = state.borrow_mut();
                    s.updater.poll();
                    s.updater.tick(now, src.as_ref(), auto, stale)
                };

                if let crate::updater::Tick::ReadyToInstall { version } = tick {
                    let mut s = state.borrow_mut();
                    // 记下这次成功检查的时刻（跨启动节流）
                    s.last_update_check = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0),
                    );
                    s.update_dialog_open = true;
                    s.shared.toast(format!("v{version} 已就绪，可以安装"));
                    s.save();
                    drop(s);
                    Self::flush_toasts(&state, &win);
                }

                let after = {
                    let s = state.borrow();
                    std::mem::discriminant(&s.updater.phase)
                };
                // 只有阶段真的变了才刷 property（下载中每秒进度会变，单独处理）
                if before != after
                    || matches!(
                        state.borrow().updater.phase,
                        crate::updater::Phase::Downloading { .. }
                    )
                {
                    Self::push_update(&state, &win);
                }
            },
        );

        let state = self.state.clone();
        let w = win.as_weak();
        let _keep = keep.clone();
        win.on_update_install(move || {
            let _hold = &_keep;
            let file = {
                let s = state.borrow();
                match &s.updater.phase {
                    crate::updater::Phase::Ready { file, .. } => Some(file.clone()),
                    _ => None,
                }
            };
            let Some(file) = file else { return };
            // 拉起安装程序并退出 —— 覆盖安装装不了正在运行的自己。
            match crate::updater::launch(&file) {
                Ok(()) => {
                    let _ = slint::quit_event_loop();
                }
                Err(e) => {
                    state.borrow_mut().shared.toast(format!("安装失败：{e}"));
                    if let Some(win) = w.upgrade() {
                        Self::flush_toasts(&state, &win);
                    }
                }
            }
        });

        let state = self.state.clone();
        let w = win.as_weak();
        win.on_update_dismiss(move || {
            state.borrow_mut().update_dialog_open = false;
            if let Some(win) = w.upgrade() {
                // 顶栏的「安装 vX」入口仍在 —— 点了「稍后」不等于丢掉这次更新。
                Self::push_update(&state, &win);
            }
        });
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
            gm: self.gm.clone(),
            ts: self.ts.clone(),
            json: self.json.clone(),
            diff: self.diff.clone(),
            market: self.market.clone(),
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
            "json-in" => Some(f(&mut self.json.borrow_mut().input)),
            "diff-left-in" => Some(f(&mut self.diff.borrow_mut().left)),
            "diff-right-in" => Some(f(&mut self.diff.borrow_mut().right)),
            // 插件的编辑区在 AppState::tools 里，不是独立字段。
            "plugin-in" => {
                let mut s = self.state.borrow_mut();
                s.active_plugin_mut().map(|p| f(&mut p.input))
            }
            "plugin-out" => {
                let mut s = self.state.borrow_mut();
                s.active_plugin_mut().map(|p| f(&mut p.output))
            }
            "rsa-pub-out" => Some(f(&mut self.rsa.borrow_mut().pub_pem)),
            "rsa-priv-out" => Some(f(&mut self.rsa.borrow_mut().priv_pem)),
            "crypto-enc-in" => Some(f(&mut self.crypto.borrow_mut().enc.input)),
            "crypto-enc-out" => Some(f(&mut self.crypto.borrow_mut().enc.output)),
            "crypto-dec-in" => Some(f(&mut self.crypto.borrow_mut().dec.input)),
            "crypto-dec-out" => Some(f(&mut self.crypto.borrow_mut().dec.output)),
            "gm-enc-in" => Some(f(&mut self.gm.borrow_mut().enc_input)),
            "gm-enc-out" => Some(f(&mut self.gm.borrow_mut().enc_output)),
            "gm-dec-in" => Some(f(&mut self.gm.borrow_mut().dec_input)),
            "gm-dec-out" => Some(f(&mut self.gm.borrow_mut().dec_output)),
            "gm-sig-in" => Some(f(&mut self.gm.borrow_mut().sig_text)),
            _ => None,
        }
    }

    /// 某个编辑区的内容改了之后，重算它所属工具的派生结果。
    fn recompute(&self, which: &str) {
        if which.starts_with("yaml-") {
            self.yaml.borrow_mut().convert();
        } else if which.starts_with("regex-") {
            self.regex.borrow_mut().run();
        } else if which.starts_with("json-") {
            self.json.borrow_mut().on_edited();
        } else if which.starts_with("diff-") {
            self.diff.borrow_mut().compare();
        } else if which.starts_with("plugin-") {
            // 插件的输入变了 → 标脏并立刻跑一次（与 egui 版一致：
            // 插件是纯计算 + 有燃料上限，实时跑得起）。
            let mut s = self.state.borrow_mut();
            if let Some(p) = s.active_plugin_mut() {
                p.on_edited();
                p.run_if_dirty();
            }
        }
    }

    /// 把编辑区所属工具的草稿落盘。
    fn persist_tool(&self, which: &str) {
        if which.starts_with("plugin-") {
            // 插件草稿按它自己的 id 存。这里直接落整份状态 ——
            // save() 会遍历 tools 收集每个工具的 save_draft()。
            self.state.borrow().save();
            return;
        }
        let (id, draft) = match which.split('-').next() {
            Some("yaml") => ("yaml", self.yaml.borrow().save_draft()),
            Some("sql") => ("sql", self.sql.borrow().save_draft()),
            Some("regex") => ("regex", self.regex.borrow().save_draft()),
            Some("rsa") => ("rsa", self.rsa.borrow().save_draft()),
            Some("crypto") => ("crypto", self.crypto.borrow().save_draft()),
            Some("gm") => ("gm", self.gm.borrow().save_draft()),
            Some("timestamp") => ("timestamp", self.ts.borrow().save_draft()),
            Some("json") => ("json", self.json.borrow().save_draft()),
            Some("diff") => ("diff", self.diff.borrow().save_draft()),
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
        } else if which.starts_with("gm-") {
            Self::push_gm(&self.gm, win);
        } else if which.starts_with("json-") {
            self.sync_json(win);
        } else if which.starts_with("diff-") {
            Self::push_diff(&self.diff, win);
        } else if which.starts_with("plugin-") {
            Self::sync_plugin(&self.state, win);
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

/// 把字节数格式成 `1.2 MB` 这种（插件卡片显示体积用）。
fn fmt_size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
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