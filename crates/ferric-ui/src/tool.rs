//! 工具抽象与共享上下文（Slint 版）。
//!
//! 与 egui 时代的关键差别：`Tool` 不再有 `ui(&mut egui::Ui)` —— 视图层搬到
//! `.slint` 里，Rust 侧只负责**状态与业务**。每个工具：
//!
//! - `meta()` 提供侧栏所需的元信息（id / 名称 / 描述 / 图标 / 分组）；
//! - `save_draft()` / `load_draft()` 维持原有的草稿持久化契约（格式不变，
//!   所以老用户的 `drafts` 数据在迁移后仍然读得出来）;
//! - `migrated()` 标记该工具的 Slint 视图是否已就绪。未就绪的在界面上显示
//!   「正在迁移」占位，但**状态与草稿照旧保留**，迁完即接上。
//!
//! 具体的输入/输出绑定由外壳（`state.rs`）按工具 id 分派到对应的 Slint property。

use serde::{Deserialize, Serialize};

/// 界面语言（轻量 i18n）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    pub fn label(self) -> &'static str {
        match self {
            Lang::Zh => "中文",
            Lang::En => "English",
        }
    }

    /// 双语取串：中文界面取 `zh`，英文取 `en`。
    pub fn pick(self, zh: &'static str, en: &'static str) -> &'static str {
        match self {
            Lang::Zh => zh,
            Lang::En => en,
        }
    }
}

/// 工具元信息（用于侧栏、搜索、标题）。
#[derive(Clone, Copy)]
pub struct ToolMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    /// Lucide 图标字符（见 `crate::icons`）。
    pub icon: char,
    /// 侧栏分组标签。
    pub group: &'static str,
    /// 搜索关键词（侧栏搜索除名称/描述外也匹配这里，命中英文名与别名）。
    pub keywords: &'static [&'static str],
}

/// 短暂提示的存活时长。
///
/// ⚠️ **必须是时间，不能是帧数** —— egui 时代这里踩过坑：帧数计时在软件光栅化
/// （8fps）的机器上会把 2 秒的提示拖成 15 秒，且那 15 秒整窗都在重画。
/// Slint 是 retained mode，不存在「提示在就满帧率转」的问题，但语义仍保持时间。
pub const TOAST_TTL: std::time::Duration = std::time::Duration::from_secs(3);

pub struct Toast {
    pub text: String,
    pub born: std::time::Instant,
}

impl Toast {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            born: std::time::Instant::now(),
        }
    }

    /// 是否已过期该移除。
    pub fn expired(&self) -> bool {
        self.born.elapsed() >= TOAST_TTL
    }
}

/// 跨工具共享的运行时上下文。
///
/// egui 时代它还带着 `Theme`（每帧传给视图画东西）；Slint 里主题是 `.slint`
/// 的 global，Rust 只需要把 `dark` 灌进去，所以这里不再持有 Theme。
#[derive(Default)]
pub struct Shared {
    pub lang: Lang,
    /// 是否为软件渲染（Slint software renderer 恒为真）。
    pub gpu_software: bool,
    /// 待显示的提示队列。
    pub toasts: Vec<Toast>,
    /// 剪贴板请求：外壳在下一次同步时消费并写进系统剪贴板。
    pub clipboard: Option<String>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            gpu_software: true,
            ..Default::default()
        }
    }

    /// 排一条提示。
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast::new(text));
    }

    /// 请求把文本写进系统剪贴板（由外壳执行 —— Slint 的剪贴板 API 需要窗口句柄）。
    pub fn copy(&mut self, text: impl Into<String>) {
        self.clipboard = Some(text.into());
    }

    /// 丢掉过期提示。外壳每次同步时调一次。
    pub fn prune_toasts(&mut self) {
        self.toasts.retain(|t| !t.expired());
    }
}

pub trait Tool {
    fn meta(&self) -> ToolMeta;

    /// 该工具的 Slint 视图是否已就绪。
    ///
    /// `false` = 界面显示「正在迁移到 Slint」占位。业务逻辑与草稿仍然完整保留，
    /// 迁完视图即可翻成 `true`，不需要动状态层。
    fn migrated(&self) -> bool {
        false
    }

    /// 若本工具是 WASM 插件，借出它。内置工具返回 `None`。
    ///
    /// 为什么不用 `Any` 向下转型：只有插件一种类型需要被外壳取回具体形态
    ///（要读 manifest 声明的选项来渲染控件），给整个 trait 加 `as_any`
    /// 是为一个用例引入一个通用后门。这两个方法把能力限定得刚好够用。
    fn as_plugin(&self) -> Option<&crate::plugin_host::PluginTool> {
        None
    }

    fn as_plugin_mut(&mut self) -> Option<&mut crate::plugin_host::PluginTool> {
        None
    }

    /// 序列化当前输入草稿以便持久化；返回 `None` 表示该工具不持久化。
    ///
    /// ⚠️ 格式与 egui 版**保持一致** —— 老用户升级后草稿不能丢。
    fn save_draft(&self) -> Option<String> {
        None
    }

    /// 从持久化字符串恢复输入草稿；数据损坏时应静默忽略（保持默认）。
    fn load_draft(&mut self, _data: &str) {}
}