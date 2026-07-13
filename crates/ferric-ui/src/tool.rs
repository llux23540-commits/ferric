//! 工具抽象与共享上下文。
//!
//! 每个工具实现 [`Tool`]，[`FerricApp`](crate::FerricApp) 持有
//! `Vec<Box<dyn Tool>>`，由此统一驱动侧边栏、搜索、收藏与路由。
//! 新增工具 = 加一个 `views/*.rs` + 在 `views::registry()` 注册一行。

use crate::theme::Theme;

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
}

impl Shared {
    pub fn new(theme: Theme) -> Self {
        Self { theme, toasts: Vec::new(), content_height: 0.0 }
    }

    /// 弹一条提示。
    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toasts.push(Toast::new(msg.into()));
    }

    /// 复制文本到剪贴板并提示。
    pub fn copy(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        let text = text.into();
        ctx.output_mut(|o| o.copied_text = text);
        self.toast("已复制");
    }
}

/// 短暂提示。以帧计时，避免依赖 `Instant`（便于跨平台与测试）。
pub struct Toast {
    pub msg: String,
    pub frames_left: u32,
}

impl Toast {
    fn new(msg: String) -> Self {
        Self { msg, frames_left: 120 }
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
