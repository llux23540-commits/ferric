//! 工具抽象与共享上下文。
//!
//! 每个工具实现 [`Tool`]，[`FerricApp`](crate::FerricApp) 持有
//! `Vec<Box<dyn Tool>>`，由此统一驱动侧边栏、搜索、收藏与路由。
//! 新增工具 = 加一个 `views/*.rs` + 在 `views::registry()` 注册一行。

use crate::theme::Theme;
use serde::{Deserialize, Serialize};

/// 界面语言（轻量 i18n）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    /// 折叠摘要的「N 节点 / N node(s)」用词（英文含复数）。
    #[allow(dead_code)]
    pub fn nodes(self, n: usize) -> String {
        match self {
            Lang::Zh => format!("{n} 节点"),
            Lang::En if n == 1 => "1 node".to_owned(),
            Lang::En => format!("{n} nodes"),
        }
    }

    /// 当前语言的短标签（用于切换按钮）。
    pub fn short(self) -> &'static str {
        match self {
            Lang::Zh => "中",
            Lang::En => "EN",
        }
    }

    /// 切换到另一种语言。
    pub fn toggled(self) -> Lang {
        match self {
            Lang::Zh => Lang::En,
            Lang::En => Lang::Zh,
        }
    }
}

/// 工具元信息（用于侧栏、搜索、标题）。
pub struct ToolMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    /// 侧栏 / 顶栏图标（Lucide 字形，见 [`crate::icons`]）。
    pub icon: char,
    pub keywords: &'static [&'static str],
}

/// 跨工具共享的运行时上下文。
pub struct Shared {
    pub theme: Theme,
    pub toasts: Vec<Toast>,
    /// 内容区可用高度（由外壳在进入滚动区前测得，供需要铺满高度的工具使用）。
    pub content_height: f32,
    /// 界面语言。
    pub lang: Lang,
    /// 当前生效的插件/更新数据源（真服务端或演示数据）。None = 两者都没有。
    /// 由外壳每帧写入，供插件市场视图使用。
    pub source: Option<crate::source::Source>,
    /// 请求外壳热加载插件（装完 / 卸完置位）。
    ///
    /// 为什么不由视图自己重建：此刻正在 `self.tools[i].ui()` 里面，
    /// 那个向量的元素正被借着，谁也不能在这里改它。外壳在本帧渲染结束后处理。
    pub reload_plugins: bool,
    /// 代码编辑区的排版（字号 / 字重 / 行距）。
    ///
    /// 放在 Shared 而不是各工具自己存：设置面板与 JSON 工具条上的字体菜单改的是
    /// **同一份**配置 —— 两个入口各存一份的话，用户从哪边改都只对一半界面生效。
    pub code_font: crate::widgets::FontCfg,
    /// 当前跑在软件光栅化（WARP / llvmpipe）上 —— 虚拟机与无驱动环境。
    /// 外壳启动时探测一次；视图可据此收敛大范围渐变这类软渲染下会糊的效果。
    pub gpu_software: bool,
}

impl Shared {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            toasts: Vec::new(),
            content_height: 0.0,
            lang: Lang::default(),
            source: None,
            reload_plugins: false,
            code_font: Default::default(),
            gpu_software: false,
        }
    }

    /// 弹一条提示。
    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toasts.push(Toast::new(msg.into()));
    }

    /// 复制文本到剪贴板并提示。
    pub fn copy(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        let text = text.into();
        ctx.copy_text(text);
        self.toast("已复制");
    }
}

/// 短暂提示。以帧计时，避免依赖 `Instant`（便于跨平台与测试）。
/// 提示条的存活时长。
///
/// ⚠️ **必须是时间，不能是帧数**。这里原本是 `frames_left: 120`，配合每帧
/// `request_repaint()`，效果是「提示在的时候界面按满帧率狂转」：
/// - 有硬件 GPU 的机器 60fps 跑完 120 帧 = 2 秒，看不出问题；
/// - 软件光栅化（Windows 的 WARP、Linux 的 llvmpipe）只跑得动 8fps，
///   同样 120 帧要 **15 秒**，而这 15 秒里整窗都在被 CPU 重画。
///
/// 也就是说机器越慢、风暴越久 —— 正反馈。这正是「Windows 打开就很卡、
/// Linux 没事」的直接来源（实测归因见 `FerricApp::toasts_ui`）。
pub const TOAST_TTL: std::time::Duration = std::time::Duration::from_secs(3);

pub struct Toast {
    pub msg: String,
    /// 到期时刻。用 `Instant` 而不是 egui 的 `input.time`：这里拿不到 `Context`，
    /// 而且单调时钟不受系统改时间影响。
    pub until: std::time::Instant,
}

impl Toast {
    fn new(msg: String) -> Self {
        Self {
            msg,
            until: std::time::Instant::now() + TOAST_TTL,
        }
    }
}

pub trait Tool {
    fn meta(&self) -> ToolMeta;
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared);

    /// 是否在顶栏下方显示描述（page-intro）。默认显示。
    fn show_desc(&self) -> bool {
        true
    }

    /// 是否铺满内容区（标题左对齐、内容不按 1080 列居中）。默认按列居中。
    fn full_bleed(&self) -> bool {
        false
    }

    /// 在顶栏标题右侧渲染的工具专属操作（如 JSON 工具条）。默认无。
    fn header_actions(&mut self, _ui: &mut egui::Ui, _shared: &mut Shared) {}

    /// 序列化当前输入草稿以便持久化；返回 `None` 表示该工具不持久化。
    fn save_draft(&self) -> Option<String> {
        None
    }

    /// 从持久化字符串恢复输入草稿；数据损坏时应静默忽略（保持默认）。
    fn load_draft(&mut self, _data: &str) {}
}
