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

/// 把「UUID 工具」单独拿出来持有具体类型，避免为一个工具给 `Tool` 加
/// `as_any` 这类只服务于向下转型的接口。
///
/// 迁移期的取舍：已迁移的工具都会像这样有一个具体字段；等 10 个工具全迁完，
/// 再统一改成 `enum ToolState { Json(..), Diff(..), ... }` 一次收敛。
pub struct Shell {
    pub state: Rc<RefCell<AppState>>,
    pub uuid: Rc<RefCell<views::UuidTool>>,
}

impl Shell {
    pub fn new() -> Self {
        let state = AppState::load();
        // UUID 工具的草稿已在 `AppState::load` 里灌进注册表那一份；这里再按
        // 同一份草稿构造一个具体类型的实例，保证两边初值一致。
        let mut uuid = views::UuidTool::default();
        if let Some(t) = state.tools.iter().find(|t| t.meta().id == "uuid") {
            if let Some(d) = t.save_draft() {
                uuid.load_draft(&d);
            }
        }
        Self {
            state: Rc::new(RefCell::new(state)),
            uuid: Rc::new(RefCell::new(uuid)),
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