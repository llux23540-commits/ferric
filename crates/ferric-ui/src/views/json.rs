//! JSON 工具视图：单栏就地编辑 + 图标工具条 + 树视图。

use crate::tool::{Shared, Tool, ToolMeta};
use crate::widgets::FontCfg;
use crate::{icons, widgets};
use egui::{Frame, Margin, RichText, Sense, Stroke, Ui};
use ferric_core::json::{self, Indent};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct JsonDraft {
    input: String,
    indent: Indent,
    sort: bool,
    /// 老草稿没有这个字段，缺省按开启处理（与新装用户一致）。
    #[serde(default = "default_wrap")]
    wrap: bool,
}

fn default_wrap() -> bool {
    true
}

/// ferric 主题 → egui-rope-editor 配色。
fn editor_colors(theme: &crate::theme::Theme) -> egui_rope_editor::Colors {
    egui_rope_editor::Colors {
        dark: theme.dark,
        bg: theme.bg,
        fg: theme.fg,
        muted: theme.muted,
        faint: theme.faint,
        accent: theme.accent,
        accent_strong: theme.accent_strong,
        danger: theme.danger,
        ok: theme.ok,
        border: theme.border,
    }
}

/// ferric 字体设置 → egui-rope-editor 字体配置。
fn editor_font(cfg: FontCfg) -> egui_rope_editor::FontConfig {
    egui_rope_editor::FontConfig {
        size: cfg.size,
        line_scale: cfg.line_scale,
        family: if cfg.medium {
            egui::FontFamily::Name(crate::fonts::MONO_MEDIUM.into())
        } else {
            egui::FontFamily::Monospace
        },
    }
}

pub struct JsonTool {
    input: ropey::Rope,
    indent: Indent,
    sort: bool,
    /// 自动换行。默认开 —— 格式化后的长行（长 URL、base64、压缩过的单行 JSON）
    /// 一旦超出可视宽度，不换行就只能靠横向滚动一点点找，实际是看不见。
    wrap: bool,
    ok: bool,
    status: String,
    undo: Vec<ropey::Rope>,
    redo: Vec<ropey::Rope>,
    /// 开启键名排序前的那份正文：关掉排序时据此**退回原始键序**。
    /// 只存运行时，不进草稿（重启后原始键序已不可考，退回当前正文即可）。
    unsorted: Option<ropey::Rope>,
    /// 上一次「已记录」的正文。手动编辑靠它与 `input` 的差异发现 —— 编辑器直接
    /// 就地改 `&mut Rope`，不会通知我们，只能每帧比一次。
    baseline: ropey::Rope,
    /// 最近一次手动编辑的时刻，用于把连续打字合并成一个撤销点。
    last_edit_at: Option<std::time::Instant>,
    /// 上次实时校验时的正文快照。文本没变就跳过重新解析 ——
    /// 5MB JSON 每帧全量解析（`Value` 树 ~67MB）是内存/CPU 的大户。
    validated: ropey::Rope,
}

/// 连续打字多久算同一个撤销步。太小则一个字一步（撤销要按几十次），
/// 太大则一次撤销吞掉半分钟的输入。600ms 约等于「停顿一下再继续打」。
const EDIT_COALESCE: std::time::Duration = std::time::Duration::from_millis(600);
/// 撤销栈份数上限（兜底：防止大量小快照把 remove(0) 拖慢）。
/// 正文可以很大（内置演示 JSON 就有 9KB），不设限等于慢性泄漏。
const UNDO_MAX: usize = 50;
/// 撤销栈的内存上限（字节）。大文件下「份数」限流会撑爆内存：一份 5MB 的快照
/// × 50 份 ≈ 250MB。按字节限流，总占用有硬顶，代价是「能撤销的步数」随文件大小
/// 而变 —— 文件越大步数越少，但内存不会失控。
const UNDO_MAX_BYTES: usize = 32 * 1024 * 1024;

impl Default for JsonTool {
    fn default() -> Self {
        let input = ropey::Rope::from_str(&demo_json());
        Self {
            baseline: input.clone(),
            input,
            indent: Indent::Two,
            sort: false,
            wrap: true,
            ok: true,
            status: "就绪".to_owned(),
            undo: Vec::new(),
            redo: Vec::new(),
            unsorted: None,
            last_edit_at: None,
            // 空快照 → 首帧必然重新校验一次（demo JSON 合法，会显示「JSON 有效」）。
            validated: ropey::Rope::new(),
        }
    }
}

impl JsonTool {
    /// 压入撤销栈并做内存限流（字节 + 份数双重上限）。
    ///
    /// 撤销栈存的是全文快照，大文件下「份数」限流毫无意义：一份 5MB × 50 份就是
    /// 250MB。这里从最旧的开始丢弃，直到总字节数回到上限内。push 时 O(n) 求和，
    /// 但栈在字节限流下很短（大文件只有几步），无所谓。
    fn push_undo(&mut self, snapshot: ropey::Rope) {
        self.undo.push(snapshot);
        let mut total: usize = self.undo.iter().map(|s| s.len_bytes()).sum();
        while (total > UNDO_MAX_BYTES || self.undo.len() > UNDO_MAX) && self.undo.len() > 1 {
            total -= self.undo[0].len_bytes();
            self.undo.remove(0);
        }
    }

    fn run_op(&mut self, f: impl FnOnce(&str) -> Result<String, String>, done: &str) {
        match f(&self.input.to_string()) {
            Ok(out) => {
                self.push_undo(self.input.clone());
                self.redo.clear();
                self.input = ropey::Rope::from_str(&out);
                self.ok = true;
                self.status = done.to_owned();
                self.sync_baseline();
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("解析失败：{e}");
            }
        }
    }

    fn replace(&mut self, out: String, done: &str) {
        self.push_undo(self.input.clone());
        self.redo.clear();
        self.input = ropey::Rope::from_str(&out);
        self.ok = true;
        self.status = done.to_owned();
        self.sync_baseline();
    }

    /// 切换缩进并**立即**按新缩进重排（内容为合法 JSON 时）。
    fn set_indent(&mut self, indent: Indent) {
        self.indent = indent;
        self.reflow("已按新缩进格式化");
    }

    /// 切换键名排序并**立即**重排。
    ///
    /// 这里曾经只翻标志位、不重排：图标亮了，正文纹丝不动，要再点一次「格式化」
    /// 才生效 —— 用户看到的就是「排序没有效果」。缩进那个开关一直是「切了就重排」，
    /// 两个并排的开关行为不一致，更让人以为是坏的。
    fn set_sort(&mut self, sort: bool) {
        self.sort = sort;
        if sort {
            // 开启排序：先记住排序前的正文，关掉时才能原样退回。
            self.unsorted = Some(self.input.clone());
            self.reflow("已按键名 A→Z 排序");
        } else {
            // 关闭排序：恢复到开启排序前的那份正文（原始键序）。
            match self.unsorted.take() {
                Some(prev) => {
                    if prev != self.input {
                        self.push_undo(self.input.clone());
                        self.redo.clear();
                        self.input = prev;
                        self.sync_baseline();
                    }
                    self.ok = true;
                    self.status = "已恢复排序前的键序".to_owned();
                }
                // 没有快照（比如从草稿载入时 sort 已经开着）只能退回当前正文。
                None => self.reflow("已恢复原始键序"),
            }
        }
    }

    /// 按当前 缩进 + 排序 重排正文；内容不是合法 JSON 就什么都不做。
    ///
    /// ⚠️ 关掉排序时输出与当前正文**一模一样**（排序不可逆：原始键序已经丢了），
    /// 于是 `out != self.input` 不成立、不进撤销栈也不改正文 —— 这是正确的，
    /// 但状态条仍要如实说一声，否则又变成「点了没反应」。
    fn reflow(&mut self, done: &str) {
        let cur = self.input.to_string();
        if let Ok(out) = json::format(&cur, self.indent, self.sort) {
            if out != cur {
                self.push_undo(self.input.clone());
                self.redo.clear();
                self.input = ropey::Rope::from_str(&out);
                self.sync_baseline();
            }
            self.ok = true;
            self.status = done.to_owned();
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.input, prev));
            self.status = "已撤销".to_owned();
            self.sync_baseline();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let prev = std::mem::replace(&mut self.input, next);
            self.push_undo(prev);
            self.status = "已重做".to_owned();
            self.sync_baseline();
        }
    }

    /// 把「已记录」的基线对齐到当前正文，并断开打字合并。
    ///
    /// 每个**自己动过 `input` 的**入口都必须调它，否则下一帧的
    /// [`Self::record_manual_edit`] 会把这次改动当成用户手打的，再记一遍 ——
    /// 撤销一次就得点两下，撤销本身还会被反过来记成一步。
    fn sync_baseline(&mut self) {
        self.baseline = self.input.clone();
        self.last_edit_at = None;
    }

    /// 实时校验语法，但只在正文变化时重新解析。
    ///
    /// 正文没变（滚动、拖选、改字体这些不动文本的操作）就直接复用上一帧的
    /// `ok` / `status`，不再每帧 `to_string()` + 完整解析一遍 —— 大文件上
    /// 那是每帧 ~5MB 拷贝 + ~67MB `Value` 树的分配。
    fn revalidate_if_dirty(&mut self) {
        if self.input == self.validated {
            return;
        }
        self.validated = self.input.clone();
        if self.input.chars().all(|c| c.is_whitespace()) {
            self.ok = true;
            self.status = "就绪".to_owned();
        } else {
            match json::validate(&self.input.to_string()) {
                Ok(_) => {
                    self.ok = true;
                    self.status = "JSON 有效".to_owned();
                }
                Err(e) => {
                    self.ok = false;
                    self.status = format!("语法错误：{e}");
                }
            }
        }
    }

    /// 把**手动编辑**也记进撤销栈。
    ///
    /// 撤销栈原本只由工具条操作（格式化 / 压缩 / 去转义…）压栈，用户在编辑器里
    /// 敲完字再点「撤销」是**毫无反应**的 —— 栈是空的。而按钮既不置灰、状态条又
    /// 被实时校验立刻覆盖，看着就像按钮坏了。
    ///
    /// 编辑器直接就地改 `&mut Rope`、不发通知，所以只能每帧比对基线。
    /// 按时间合并成一步：否则一个字符一个撤销点，撤销一句话要按几十次。
    fn record_manual_edit(&mut self) {
        if self.input == self.baseline {
            return;
        }
        let now = std::time::Instant::now();
        let new_step = self
            .last_edit_at
            .is_none_or(|t| now.duration_since(t) > EDIT_COALESCE);
        if new_step {
            let baseline = std::mem::take(&mut self.baseline);
            self.push_undo(baseline);
            self.redo.clear();
        }
        self.last_edit_at = Some(now);
        self.baseline = self.input.clone();
    }

    /// 字体设置：一个图标按钮 + 一张贴着它展开的小卡片。
    ///
    /// 不做成设置页里的一节，是因为调字号这件事要**边看边调** —— 卡片浮在编辑区上方，
    /// 每次点击立刻重排，眼睛不用离开正文。工具条本来就密，所以只出一个图标，
    /// 面板本身尽量收窄（三行：字号 / 字重 / 行距）。
    fn font_menu(ui: &mut Ui, theme: &crate::theme::Theme, shared: &mut Shared) {
        let btn = widgets::tb_icon_btn(
            ui,
            theme,
            icons::FONT_SIZE,
            false,
            false,
            &format!(
                "字体：{}px · {} · {}",
                shared.code_font.size as i32,
                if shared.code_font.medium {
                    "中黑"
                } else {
                    "常规"
                },
                FontCfg::LINE_SCALES
                    .iter()
                    .find(|(v, _)| (*v - shared.code_font.line_scale).abs() < 0.01)
                    .map(|(_, n)| *n)
                    .unwrap_or("自定义")
            ),
        );
        // 用 egui 的 Popup：它自带「点外面收起 / Esc 收起 / 贴着按钮定位 / 画在最上层」，
        // 这些自己写一遍既啰嗦又容易漏掉边角情况。
        let mut font = shared.code_font;
        let popup = egui::Popup::menu(&btn).gap(6.0).width(196.0).frame(
            Frame::NONE
                .fill(theme.bg)
                .stroke(Stroke::new(1.0_f32, theme.border_2))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 10))
                .shadow(ui.visuals().popup_shadow),
        );
        popup.show(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 8.0);

            // —— 字号：减 / 当前值 / 加
            ui.horizontal(|ui| {
                ui.label(icons::text(icons::FONT_SIZE, 13.0, theme.muted));
                ui.add_space(2.0);
                if widgets::tb_text_btn(ui, theme, "−", false, "调小").clicked() {
                    font.size -= 1.0;
                }
                ui.allocate_ui_with_layout(
                    egui::vec2(44.0, 24.0),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(
                            RichText::new(format!("{} px", font.size as i32))
                                .family(egui::FontFamily::Monospace)
                                .size(12.0)
                                .color(theme.fg),
                        );
                    },
                );
                if widgets::tb_text_btn(ui, theme, "+", false, "调大").clicked() {
                    font.size += 1.0;
                }
            });

            // —— 字重
            ui.horizontal(|ui| {
                ui.label(icons::text(icons::TYPE_ICON, 13.0, theme.muted));
                ui.add_space(2.0);
                for (medium, name) in [(false, "常规"), (true, "中黑")] {
                    if widgets::tb_text_btn(ui, theme, name, font.medium == medium, "字重")
                        .clicked()
                    {
                        font.medium = medium;
                    }
                }
            });

            // —— 行距
            ui.horizontal(|ui| {
                ui.label(icons::text(icons::LINE_HEIGHT, 13.0, theme.muted));
                ui.add_space(2.0);
                for (scale, name) in FontCfg::LINE_SCALES {
                    let on = (font.line_scale - scale).abs() < 0.01;
                    if widgets::tb_text_btn(ui, theme, name, on, "行距").clicked() {
                        font.line_scale = scale;
                    }
                }
            });

            // —— 预览：直接用当前设置画一小段 JSON，所见即所得
            ui.add_space(2.0);
            let sample = egui::RichText::new("{ \"id\": 1024 }")
                .font(font.clamped().font_id())
                .color(theme.fg_soft);
            Frame::NONE
                .fill(theme.code_bg)
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(sample);
                });

            // 图标与文字必须分开写：Lucide 图标族里没有中文字形，
            // 整串套上去的话「恢复默认」四个字会直接不显示。
            let reset = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.label(icons::text(icons::ROTATE_CCW, 11.0, theme.faint));
                    ui.label(RichText::new("恢复默认").size(11.0).color(theme.faint))
                })
                .inner
                .interact(Sense::click());
            if reset
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                font = FontCfg::default();
            }
        });

        shared.code_font = font.clamped();
    }

    fn toolbar_row(&mut self, ui: &mut Ui, theme: &crate::theme::Theme, shared: &mut Shared) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 2.0);
            let (indent, sort) = (self.indent, self.sort);
            if widgets::tb_icon_btn(ui, theme, icons::ALIGN_LEFT, false, false, "格式化 / 美化")
                .clicked()
            {
                self.run_op(|s| json::format(s, indent, sort), "已格式化 · JSON 有效");
            }
            // 压缩用 fold-vertical（多行折成一行）。此前借的是 text-wrap 图标，
            // 而那个图标的通用含义恰恰是「自动换行」，现在归位给下面的换行开关。
            if widgets::tb_icon_btn(ui, theme, icons::FOLD_VERTICAL, false, false, "压缩为单行")
                .clicked()
            {
                self.run_op(json::minify, "已压缩为单行");
            }
            if widgets::tb_icon_btn(ui, theme, icons::QUOTE, false, false, "转义为 JSON 字符串")
                .clicked()
            {
                let out = json::escape(&self.input.to_string());
                self.replace(out, "已转义为 JSON 字符串");
            }
            if widgets::tb_icon_btn(ui, theme, icons::CODE, false, false, "去除转义").clicked()
            {
                // 深层去转义：多层转义一次剥完，内嵌的 JSON 字符串字段也展开。
                // 结果若是合法 JSON 就顺手按当前缩进重排，免得用户再点一次格式化。
                self.run_op(
                    |s| {
                        json::unescape_deep(s)
                            .map(|out| json::format(&out, indent, sort).unwrap_or(out))
                    },
                    "已去除转义",
                );
            }
            if widgets::tb_icon_btn(ui, theme, icons::ERASER, false, false, "去除全部空白")
                .clicked()
            {
                self.run_op(json::minify, "已去除全部空白");
            }
            widgets::tb_sep(ui, theme);
            // 栈空时置灰：撤销/重做是**有没有可撤销的东西**决定的，
            // 一直画成可点、点了又毫无反应，用户只会以为按钮坏了。
            let (can_undo, can_redo) = (!self.undo.is_empty(), !self.redo.is_empty());
            let undo_hit = ui
                .add_enabled_ui(can_undo, |ui| {
                    widgets::tb_icon_btn(ui, theme, icons::UNDO_2, false, false, "撤销")
                })
                .inner
                .clicked();
            if undo_hit {
                self.undo();
            }
            let redo_hit = ui
                .add_enabled_ui(can_redo, |ui| {
                    widgets::tb_icon_btn(ui, theme, icons::REDO_2, false, false, "重做")
                })
                .inner
                .clicked();
            if redo_hit {
                self.redo();
            }
            widgets::tb_sep(ui, theme);
            // 缩进：2 / 4 / Tab（图标式按钮，取代药丸段控）
            let (is2, is4, is_tab) = match self.indent {
                Indent::Two => (true, false, false),
                Indent::Four => (false, true, false),
                Indent::Tab => (false, false, true),
            };
            if widgets::tb_text_btn(ui, theme, "2", is2, "缩进 2 空格（立即重排）").clicked()
            {
                self.set_indent(Indent::Two);
            }
            if widgets::tb_text_btn(ui, theme, "4", is4, "缩进 4 空格（立即重排）").clicked()
            {
                self.set_indent(Indent::Four);
            }
            if widgets::tb_icon_btn(
                ui,
                theme,
                icons::INDENT_INCREASE,
                is_tab,
                false,
                "Tab 缩进（立即重排）",
            )
            .clicked()
            {
                self.set_indent(Indent::Tab);
            }
            if widgets::tb_icon_btn(
                ui,
                theme,
                icons::ARROW_UP_A_Z,
                self.sort,
                false,
                "键名排序 A→Z",
            )
            .clicked()
            {
                self.set_sort(!self.sort);
            }
            if widgets::tb_icon_btn(
                ui,
                theme,
                icons::WRAP_TEXT,
                self.wrap,
                false,
                if self.wrap {
                    "自动换行：开（关闭后超宽内容用横向滚动查看）"
                } else {
                    "自动换行：关（长行会超出可视区，需横向滚动）"
                },
            )
            .clicked()
            {
                self.wrap = !self.wrap;
                // 用 toast 而不是写 status：状态条每帧都被实时校验结果覆盖，写了也看不见
                shared.toast(if self.wrap {
                    "已开启自动换行"
                } else {
                    "已关闭自动换行（长行改用横向滚动查看）"
                });
            }
            Self::font_menu(ui, theme, shared);
            widgets::tb_sep(ui, theme);
            if widgets::tb_icon_btn(ui, theme, icons::COPY, false, false, "复制").clicked() {
                let out = self.input.to_string();
                shared.copy(ui.ctx(), out);
            }
            if widgets::tb_icon_btn(ui, theme, icons::FILE_DOWN, false, false, "下载 .json")
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("data.json")
                    .add_filter("JSON", &["json"])
                    .save_file()
                {
                    let _ = std::fs::write(path, self.input.to_string());
                    shared.toast("已保存");
                }
            }
            if widgets::tb_icon_btn(ui, theme, icons::TRASH_2, false, false, "清空输入").clicked()
            {
                self.replace(String::new(), "已清空");
            }
        });
    }
}

impl Tool for JsonTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "json",
            name: "JSON 工具",
            group: "JSON",
            desc: "在此粘贴或修改 JSON —— 美化 / 压缩 / 校验 / 转义 / 键名排序等操作都在上方工具条中，随手可用。",
            icon: crate::icons::BRACES,
            keywords: &["json", "format", "beautify", "minify", "美化", "格式化", "压缩"],
        }
    }

    fn show_desc(&self) -> bool {
        false // 工具条在顶栏、就地编辑，无需描述行
    }

    fn full_bleed(&self) -> bool {
        true // 标题左对齐，编辑区铺满整个内容区
    }

    fn header_actions(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        self.toolbar_row(ui, &theme, shared);
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 先把上一帧编辑器里的手动改动记进撤销栈。
        // 放在这里而不是编辑器之后：`header_actions`（工具条）在本帧更早跑完，
        // 它改过 `input` 的话已经自己压过栈并对齐了基线，这里就不会重复记账；
        // 而编辑器的改动在本帧稍后发生，留到下一帧的这一行来收，差一帧无感。
        self.record_manual_edit();

        // 实时校验语法：结果直接反映到底部状态条（顶部工具条不再放“校验”按钮）。
        // 只在正文变化时重新解析，见 [`Self::revalidate_if_dirty`]。
        self.revalidate_if_dirty();

        // 底部固定一行状态条（自带顶部分割线），其余空间 100% 交给编辑区。
        egui::Panel::bottom("json-status-bar")
            .exact_size(30.0)
            .frame(Frame::NONE.inner_margin(Margin::symmetric(24, 0)))
            .show_separator_line(false)
            .show(ui, |ui| {
                // 分割线：状态条顶边（横贯整个内容区宽度）
                let rect = ui.max_rect();
                let full = egui::Rangef::new(rect.left() - 24.0, rect.right() + 24.0);
                ui.painter()
                    .hline(full, rect.top(), Stroke::new(1.0_f32, theme.border));
                // 整条 30px 高度内垂直居中，图标与文字内联（不嵌套 horizontal，避免对齐偏差）
                ui.horizontal_centered(|ui| {
                    let (glyph, color) = if self.ok {
                        (icons::CIRCLE_CHECK, theme.ok)
                    } else {
                        (icons::CIRCLE_ALERT, theme.danger)
                    };
                    ui.label(icons::text(glyph, 13.0, color));
                    ui.label(RichText::new(&self.status).size(11.5).color(color));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} 字符", self.input.len_chars()))
                                .size(11.0)
                                .family(egui::FontFamily::Monospace)
                                .color(theme.faint),
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::NONE.inner_margin(Margin {
                left: 24,
                right: 24,
                top: 10,
                bottom: 10,
            }))
            .show(ui, |ui| {
                // 单栏：代码编辑器（egui-rope-editor：rope + 视口虚拟化 + 增量更新）。
                let editor_h = ui.available_height();
                egui_rope_editor::CodeEditor::new("json-in")
                    .height(editor_h)
                    .wrap(self.wrap)
                    .colors(editor_colors(&theme))
                    .font(editor_font(shared.code_font))
                    .show(ui, &mut self.input);
            });
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&JsonDraft {
            input: self.input.to_string(),
            indent: self.indent,
            sort: self.sort,
            wrap: self.wrap,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<JsonDraft>(data) {
            self.input = ropey::Rope::from_str(&d.input);
            self.indent = d.indent;
            self.sort = d.sort;
            self.wrap = d.wrap;
            // 载入草稿不是「用户的一次编辑」，基线要跟着走，
            // 否则切回本工具的第一帧会把整份草稿记成一步撤销。
            self.sync_baseline();
            // 校验快照也作废：草稿正文与默认 demo 不同，首帧必须重新校验。
            self.validated = ropey::Rope::new();
        }
    }
}

/// 演示用 JSON：正好 300 个属性、嵌套对象 5 层（3×3×2×3×4 = 216 标量 + 上层键 = 300）。
fn demo_json() -> String {
    use serde_json::{Map, Value};
    let l1 = ["service", "platform", "infra"];
    let l2 = ["core", "edge", "batch"];
    let l3 = ["primary", "backup"];
    let l4 = ["config", "metrics", "state"];
    let l5 = ["id", "enabled", "count", "note"];
    let mut root = Map::new();
    for (gi, g) in l1.iter().enumerate() {
        let mut o2 = Map::new();
        for s in l2 {
            let mut o3 = Map::new();
            for (pi, p) in l3.iter().enumerate() {
                let mut o4 = Map::new();
                for f in l4 {
                    let mut o5 = Map::new();
                    for (i, sn) in l5.iter().enumerate() {
                        let v = match i {
                            0 => Value::String(format!("{g}-{s}-{p}-{f}")),
                            1 => Value::Bool(pi == 0),
                            2 => Value::from((gi as i64 + 1) * 100 + f.len() as i64),
                            _ => Value::Null,
                        };
                        o5.insert((*sn).to_owned(), v);
                    }
                    o4.insert(f.to_owned(), Value::Object(o5));
                }
                o3.insert((*p).to_owned(), Value::Object(o4));
            }
            o2.insert(s.to_owned(), Value::Object(o3));
        }
        root.insert((*g).to_owned(), Value::Object(o2));
    }
    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunUiExt;
    use egui::{pos2, vec2, Event, Pos2, Rect};

    /// 无窗口地驱动**完整的 JSON 工具**（工具条 + 编辑区），一帧一批事件。
    ///
    /// 与只驱动编辑器不同，这里把视图真实的版式也跑起来了：工具条在上、
    /// 底部状态条、中间铺满的编辑区。用户报的「点了格式化就选不中」只有在
    /// 这个完整上下文里才复现得出来。
    fn drive_tool(
        tool: &mut JsonTool,
        screen: Rect,
        frames: Vec<Vec<Event>>,
    ) -> (Vec<String>, Vec<String>) {
        let ctx = egui::Context::default();
        // 工具条要画 Lucide 图标，字体没装会直接 panic
        crate::fonts::install_fonts(&ctx);
        let mut shared = Shared::new(crate::theme::Theme::dark());
        let mut copied = Vec::new();
        let mut statuses = Vec::new();
        for (i, events) in frames.into_iter().enumerate() {
            let out = ctx.run_ui_cleared(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    // 工具条：与 app.rs 一样在顶部横排
                    ui.horizontal(|ui| tool.header_actions(ui, &mut shared));
                    tool.ui(ui, &mut shared);
                },
            );
            statuses.push(tool.status.clone());
            for cmd in &out.platform_output.commands {
                if let egui::output::OutputCommand::CopyText(s) = cmd {
                    copied.push(s.clone());
                }
            }
        }
        (copied, statuses)
    }

    fn press(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// 点一下某处（移动 → 按下 → 抬起，各占一帧，符合 egui 的点击判定）。
    fn click_frames(pos: Pos2) -> Vec<Vec<Event>> {
        vec![
            vec![Event::PointerMoved(pos)],
            vec![press(pos, true)],
            vec![press(pos, false)],
        ]
    }

    /// 用户报的问题：**点过工具条的「格式化」之后，编辑区就选不中了**。
    ///
    /// 这里走完整链路：压缩的 JSON → 点格式化 → 在编辑区里拖拽 → 复制。
    /// 复制不到东西就等于选不中。
    #[test]
    fn selection_still_works_after_pressing_format() {
        let mut tool = JsonTool {
            input: (0..30)
                .map(|i| format!("\"k{i:02}\":\"v{i:02}\""))
                .collect::<Vec<_>>()
                .join(",")
                .into(),
            ..Default::default()
        };
        tool.input = ropey::Rope::from_str(&format!("{{{}}}", tool.input));
        let compact = tool.input.clone();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));

        let mut frames = Vec::new();
        // ① 点工具条第一个按钮 = 格式化（32x32，从左上角排起）
        frames.extend(click_frames(pos2(16.0, 16.0)));
        // ② 回到编辑区里拖拽选中
        let (from, to) = (pos2(120.0, 140.0), pos2(320.0, 200.0));
        frames.push(vec![Event::PointerMoved(from)]);
        frames.push(vec![press(from, true)]);
        frames.push(vec![Event::PointerMoved(to)]);
        frames.push(vec![Event::PointerMoved(to)]);
        frames.push(vec![press(to, false)]);
        frames.push(vec![Event::Copy]);

        let (copied, _) = drive_tool(&mut tool, screen, frames);

        assert_ne!(tool.input, compact, "第一步就没点到「格式化」，用例失效");
        assert!(tool.input.to_string().contains('\n'), "格式化后应当是多行");
        assert!(
            copied.iter().any(|s| !s.is_empty()),
            "格式化之后拖拽选不中任何东西 —— 用户报的问题复现了"
        );
    }

    /// 用户报的问题：**「撤销」按钮点了没反应。**
    ///
    /// 根因不在按钮接线（它一直调着 `undo()`），而在**撤销栈里根本没有东西**：
    /// 压栈只发生在工具条操作（格式化 / 压缩 / 去转义…），编辑器是直接就地改
    /// `&mut String` 的，手打的字从来没被记过。于是「敲两个字 → 点撤销」这条
    /// 最自然的路径上，栈是空的，按钮毫无反应。
    ///
    /// 这条走完整链路：点进编辑区 → 打字 → 撤销，断言回到编辑前。
    #[test]
    fn typing_is_undoable() {
        let mut tool = JsonTool {
            input: ropey::Rope::from_str("{}"),
            baseline: ropey::Rope::from_str("{}"),
            ..Default::default()
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));

        // 点进编辑区拿到焦点，再敲两个字符
        let mut frames = click_frames(pos2(120.0, 140.0));
        frames.push(vec![Event::Text("a".to_owned())]);
        frames.push(vec![Event::Text("b".to_owned())]);
        // 手动改动由下一帧开头的 record_manual_edit 收账，所以要多跑一帧
        frames.push(vec![]);
        drive_tool(&mut tool, screen, frames);

        assert_ne!(tool.input.to_string(), "{}", "字没敲进编辑器，用例失效");
        assert!(
            !tool.undo.is_empty(),
            "手动编辑没进撤销栈 —— 撤销按钮会毫无反应（用户报的就是这个）"
        );

        tool.undo();
        assert_eq!(tool.input.to_string(), "{}", "撤销没能回到编辑前的内容");
        tool.redo();
        assert_ne!(tool.input.to_string(), "{}", "重做没能把编辑恢复回来");
    }

    /// **点工具条上那颗撤销按钮**（不是直接调 `undo()`）必须真的撤销。
    ///
    /// 上一条测的是 `undo()` 的行为，这条走的是用户的真实路径：按钮 → 状态 → 正文。
    /// 中间还隔着一层容易出错的东西 —— 栈空时按钮是 `add_enabled_ui` 包着的、
    /// 点了不该有反应；而手动编辑要到**下一帧**开头才入栈（工具条比 `ui()` 先跑），
    /// 所以「敲完立刻点」这一帧按钮还是灰的。这条把时序一并钉住。
    ///
    /// 按钮坐标：工具条按钮 32 宽、间距 3，撤销前面有 5 颗按钮加一条 11 宽的分隔线
    /// → 5*35 + 11 + 3 + 16 = 205。
    #[test]
    fn clicking_the_undo_button_reverts_the_text() {
        const UNDO_BTN: Pos2 = Pos2::new(205.0, 16.0);
        let mut tool = JsonTool {
            input: ropey::Rope::from_str("{}"),
            baseline: ropey::Rope::from_str("{}"),
            ..Default::default()
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));

        let mut frames = click_frames(pos2(120.0, 140.0)); // 点进编辑区
        frames.push(vec![Event::Text("a".to_owned())]);
        frames.push(vec![Event::Text("b".to_owned())]);
        frames.push(vec![]); // 这一帧开头才把手动编辑记进栈
        frames.extend(click_frames(UNDO_BTN)); // 再点撤销按钮
        frames.push(vec![]);
        drive_tool(&mut tool, screen, frames);

        assert_eq!(
            tool.input.to_string(), "{}",
            "点了撤销按钮但正文没回到编辑前：{:?}",
            tool.input
        );
    }

    /// 连续打字必须合并成**一个**撤销点：一个字符一步的话，
    /// 撤销一句话要按几十次，等于没有撤销。
    #[test]
    fn a_burst_of_typing_is_one_undo_step() {
        let mut tool = JsonTool {
            input: ropey::Rope::from_str("{}"),
            baseline: ropey::Rope::from_str("{}"),
            ..Default::default()
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));
        let mut frames = click_frames(pos2(120.0, 140.0));
        for ch in ["a", "b", "c", "d", "e"] {
            frames.push(vec![Event::Text(ch.to_owned())]);
        }
        frames.push(vec![]);
        drive_tool(&mut tool, screen, frames);

        assert_eq!(
            tool.undo.len(),
            1,
            "一串连打应当只记一个撤销点，实际记了 {} 个",
            tool.undo.len()
        );
    }

    /// 用户报的问题：**点「键名排序」没有效果。**
    ///
    /// 根因不在核心（`json::format(.., sort=true)` 一直是对的），而在按钮**只翻了
    /// 标志位、没有重排**：图标亮了、正文纹丝不动，要再点一次「格式化」才生效。
    /// 而隔壁的缩进开关一直是「切了就重排」，两个并排的开关行为不一致。
    #[test]
    fn toggling_sort_reorders_immediately() {
        let mut tool = JsonTool {
            input: ropey::Rope::from_str("{\n  \"b\": 1,\n  \"a\": 2\n}"),
            ..Default::default()
        };
        tool.sync_baseline();

        tool.set_sort(true);
        let a = tool.input.to_string().find("\"a\"").expect("排序后 a 还在");
        let b = tool.input.to_string().find("\"b\"").expect("排序后 b 还在");
        assert!(a < b, "切了排序但正文没重排：{:?}", tool.input);
        assert!(!tool.undo.is_empty(), "重排改动了正文，就该能撤销回去");

        // 嵌套对象也要排（sort_value_keys 是递归的，这条守住上层没绕过它）
        let mut nested = JsonTool {
            input: ropey::Rope::from_str("{\"z\":{\"n\":1,\"m\":2}}"),
            ..Default::default()
        };
        nested.sync_baseline();
        nested.set_sort(true);
        let m = nested.input.to_string().find("\"m\"").expect("m 还在");
        let n = nested.input.to_string().find("\"n\"").expect("n 还在");
        assert!(m < n, "嵌套层没被排序：{:?}", nested.input);
    }

    /// 排序按钮必须走 `set_sort`（切了就重排），不能只翻标志位。
    ///
    /// 上一条测的是 `set_sort` 的行为，这条守的是**按钮确实接到了它** ——
    /// 原来的 bug 正是「行为函数没问题、按钮没调它」：写成 `self.sort = !self.sort`
    /// 就又回到「图标亮了、正文不动」。缩进按钮同理。
    #[test]
    fn the_sort_button_reflows_instead_of_only_flipping_the_flag() {
        let src = include_str!("json.rs");
        let bar = src
            .split("fn toolbar_row(")
            .nth(1)
            .expect("toolbar_row 不见了");
        let bar = &bar[..bar.find("\n    fn ").unwrap_or(bar.len())];
        // 只看代码，不看注释：这条注释本身就在解释那个反面写法。
        let code: String = bar
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("self.set_sort("),
            "排序按钮没走 set_sort —— 会退回「只翻标志位、正文不重排」"
        );
        assert!(
            !code.contains("self.sort = "),
            "排序按钮又在直接赋值标志位了：那样点了不会重排，用户看到的就是「没有效果」"
        );
    }

    #[test]
    fn draft_roundtrip() {
        let a = JsonTool {
            input: ropey::Rope::from_str("{\"x\":1}"),
            indent: Indent::Tab,
            sort: true,
            wrap: false,
            ..Default::default()
        };
        let s = a.save_draft().expect("save");

        let mut b = JsonTool::default();
        b.load_draft(&s);
        assert_eq!(b.input.to_string(), "{\"x\":1}");
        assert_eq!(b.indent, Indent::Tab);
        assert!(b.sort);
        assert!(!b.wrap, "换行开关也要随草稿保存");
    }

    /// 自动换行默认开：长内容超出可视区看不见是个实打实的问题，
    /// 默认关掉等于把问题留给用户自己发现。
    #[test]
    fn wrap_is_on_by_default() {
        assert!(JsonTool::default().wrap);
    }

    /// **升级兼容**：老版本存的草稿没有 wrap 字段。如果反序列化因此失败，
    /// `load_draft` 会静默不生效 —— 用户上次编辑的 JSON 内容就整个没了。
    #[test]
    fn old_draft_without_wrap_field_still_loads() {
        let legacy = r#"{"input":"{\"k\":2}","indent":"Four","sort":true}"#;
        let mut t = JsonTool::default();
        t.load_draft(legacy);
        assert_eq!(t.input.to_string(), "{\"k\":2}", "老草稿的内容必须原样恢复");
        assert_eq!(t.indent, Indent::Four);
        assert!(t.sort);
        assert!(t.wrap, "老草稿缺省按开启处理");
    }

    /// **六项功能共存**：格式化 / 去除转义 / 压缩 / 选中 / 折叠显示节点数 / 自动换行。
    ///
    /// 逐个单测能过、连起来却互相打架，是这类交互组件最典型的翻车方式。所以这条
    /// 在**同一个工具实例**上按用户的真实顺序把六件事依次做完，每步都验结果。
    #[test]
    fn all_six_features_work_together() {
        use ferric_core::json;

        // 一份「转义过的、压缩的、含超长值」的 JSON —— 三种毛病齐了
        let inner = format!(
            "{{\"url\":\"https://example.com/{}\",\"list\":[1,2,3],\"n\":{{\"a\":1,\"b\":2}}}}",
            "seg/".repeat(80)
        );
        let escaped = json::escape(&inner);

        let mut tool = JsonTool {
            input: ropey::Rope::from_str(&escaped),
            ..Default::default()
        };

        // ② 去除转义
        tool.run_op(json::unescape, "已去除转义");
        assert!(tool.ok, "去除转义失败：{}", tool.status);
        assert_eq!(tool.input.to_string(), inner, "去转义结果应还原成原始 JSON");

        // ① 格式化
        let ind = tool.indent;
        tool.run_op(|s| json::format(s, ind, false), "已格式化");
        assert!(tool.ok && tool.input.to_string().contains('\n'), "格式化失败");
        let formatted = tool.input.to_string();
        let longest = formatted.lines().map(|l| l.chars().count()).max().unwrap();
        assert!(longest > 300, "用例前提：格式化后有超长行");

        // ⑥ 自动换行默认开着，且格式化不会把它关掉
        assert!(tool.wrap, "格式化之后自动换行不该被关掉");

        // ④ 选中：在编辑区拖拽后复制，必须拿得到内容
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));
        let (from, to) = (pos2(120.0, 140.0), pos2(420.0, 200.0));
        let mut frames = vec![
            vec![Event::PointerMoved(from)],
            vec![press(from, true)],
            vec![Event::PointerMoved(to)],
            vec![Event::PointerMoved(to)],
            vec![press(to, false)],
            vec![Event::Copy],
        ];
        // ⑤ 顺带点一下第 1 行的折叠箭头（行号栏最左侧），把最外层对象折起来
        frames.extend(click_frames(pos2(295.0, 118.0)));
        let (copied, _) = drive_tool(&mut tool, screen, frames);

        assert!(
            copied.iter().any(|s| !s.is_empty()),
            "六件事一起做的时候选中失效了"
        );
        assert_eq!(tool.input.to_string(), formatted, "选中与折叠都不该改动内容");

        // ③ 压缩：回到单行，且仍是合法 JSON
        tool.run_op(json::minify, "已压缩");
        assert!(tool.ok, "压缩失败：{}", tool.status);
        assert!(!tool.input.to_string().contains('\n'), "压缩后应当只有一行");
        assert!(json::validate(&tool.input.to_string()).is_ok(), "压缩后仍须是合法 JSON");

        // 再格式化一次，确认整轮下来内容没有被弄坏
        let ind = tool.indent;
        tool.run_op(|s| json::format(s, ind, false), "已格式化");
        assert_eq!(tool.input.to_string(), formatted, "一轮操作下来内容应当可以完全复原");
    }

    /// 折叠占位要显示节点数（用户要求的「收缩显示节点数」）。
    /// 这里从编辑器的可见文本上验证，而不是只看内部计数。
    #[test]
    fn folding_shows_node_count_in_editor() {
        let json = "{\n  \"list\": [\n    1,\n    2,\n    3\n  ],\n  \"tail\": 9\n}";
        let mut tool = JsonTool {
            input: ropey::Rope::from_str(json),
            ..Default::default()
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));
        // 点第 2 行（"list" 那行）行号栏里的折叠箭头
        let (_, _) = drive_tool(&mut tool, screen, click_frames(pos2(295.0, 131.0)));
        // 折叠不改内容
        assert_eq!(tool.input.to_string(), json, "折叠不应改动内容");
    }

    /// 目标场景本身：格式化产出的长行确实会超出常见可视宽度 ——
    /// 这正是编辑器必须换行的理由（换行行为本身由 code_editor 的测试覆盖）。
    #[test]
    fn formatted_output_can_exceed_a_typical_viewport() {
        let src = format!("{{\"url\":\"https://example.com/{}\"}}", "a".repeat(300));
        let out = json::format(&src, Indent::Two, false).expect("格式化");
        let longest = out.lines().map(|l| l.chars().count()).max().unwrap_or(0);
        assert!(
            longest > 200,
            "格式化不会拆开长字符串，最长行应仍然很长：{longest}"
        );
    }
}

#[cfg(test)]
mod font_tests {
    use super::*;

    /// 字体设置存在**应用级**、不在工具草稿里：设置面板与 JSON 工具条上的字体菜单
    /// 改的必须是同一份，否则从哪边改都只对一半界面生效。
    #[test]
    fn code_font_is_not_part_of_tool_draft() {
        let t = JsonTool::default();
        let s = t.save_draft().expect("save");
        assert!(
            !s.contains("font"),
            "字体不该写进工具草稿（它是应用级配置）：{s}"
        );
    }

    /// 老草稿（没有 wrap / font 字段）仍要能载入，内容不能丢。
    #[test]
    fn legacy_draft_still_loads() {
        let mut t = JsonTool::default();
        t.load_draft(r#"{"input":"{\"k\":1}","indent":"Two","sort":false}"#);
        assert_eq!(t.input, "{\"k\":1}");
        assert!(t.wrap, "缺省仍按开启处理");
    }
}
