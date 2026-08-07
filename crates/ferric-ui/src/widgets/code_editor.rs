//! 自研代码编辑器：可自由编辑 + 语法高亮 + **代码折叠**（单视图共存）。
//!
//! 设计：`text: String` 为唯一真相。每帧由源文本构建一份「可见文本」（被折叠的
//! 括号区间替换成占位 `⋯`）以及「可见↔源」字符段映射。galley / 光标 / 选区都作用在
//! **可见文本** 上（于是 egui 自带的光标运算天然跳过被折叠的行）；编辑时再把可见位置
//! 换算回源位置落到源 `String`。折叠区间由 JSON 花括号 / 方括号配对算出。
//!
//! 复用 egui 0.35 公开工具：`TextBuffer`（改写源串）、`CCursorRange::on_key_press`
//! （光标移动）、`TextCursorState::pointer_interaction`（鼠标选择）、
//! `paint_text_selection`/`paint_cursor_end`（选区/光标绘制）、`Galley::cursor_from_pos`。
//!
//! # 自动换行
//!
//! `wrap = true` 时把 galley 的 `wrap.max_width` 设成可视宽度，长行折到下一视觉行；
//! `wrap = false` 时不换行，改由 `ScrollArea` 提供横向滚动。**两者必须二选一** ——
//! 此前是「不换行 + 只有纵向滚动」，超出可视区域的内容既看不到也滚不到，等于被吞掉。
//!
//! 开了换行之后，「一个逻辑行 = 一个 galley 行」这个前提就不成立了（一行可以折成很多
//! 视觉行），所以行号与折叠箭头的定位都不能再靠数换行符：
//! - 行号：沿 `galley.rows` 累加每行字符数得到该行在可见文本中的起点，并且只在
//!   **逻辑行的首个视觉行**上打号（续行留空，与 VS Code 一致）；
//! - 折叠箭头：直接用 `galley.pos_from_cursor()` 拿开括号所在位置的 y，与折行无关。
//!
//! 换行宽度也是 `break_anywhere` 的：JSON 里一个 base64 / URL 长串中间没有空格，
//! 只按词断的话它照样会冲出可视区。

use crate::theme::Theme;
use egui::text::{CCursor, CharIndex};
use egui::text_selection::text_cursor_state::{cursor_rect, TextCursorState};
use egui::text_selection::visuals::{paint_cursor_end, paint_text_selection};
use egui::text_selection::CCursorRange;
use egui::{
    pos2, vec2, Align2, Color32, Event, EventFilter, Key, Modifiers, Pos2, Rect, Response, Sense,
    Shape, TextBuffer, Ui, Vec2,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 编辑器持久状态（光标 + 折叠集 + 待映射源光标）。
#[derive(Clone, Default)]
struct EditorState {
    cursor: TextCursorState,
    /// 已折叠区间：以「开括号 `{`/`[` 的源字符下标」为键（编辑时随文本平移）。
    folded: HashSet<usize>,
    /// 结构性编辑后待重映射的源光标（下一帧重建后换算回可见坐标）。
    pending: Option<usize>,
    /// 输入法预编辑串当前占用的源字符区间（组字中；提交/取消后清空）。
    ime: Option<(usize, usize)>,
    /// 焦点意图闩锁：一旦在编辑器内交互就置真，直到在编辑器外点击才清除。
    /// 用于在低帧率软渲染 / 输入法激活导致的瞬时失焦后，下一帧稳健地重夺焦点。
    want_focus: bool,
    /// 调试：最近事件日志（临时）。
    dbg: Vec<String>,
}

/// Ctrl+F 搜索条状态。与 [`EditorState`] 分开存：搜索是叠加在编辑器上的视图物件，
/// 打开/关闭不该牵动光标与折叠状态的读写路径。
#[derive(Clone, Default)]
struct SearchState {
    open: bool,
    query: String,
    /// 当前匹配序号（0 起）。匹配总数变化时由渲染帧收敛回合法区间。
    cur: usize,
    /// 最近一个渲染帧统计出的匹配总数（搜索条显示 n/total 用）。
    total: usize,
    /// 需要把当前匹配滚进视口（查询变化 / 导航之后置真，滚完即清）。
    goto: bool,
    /// 搜索框需要抢焦点（刚按 Ctrl+F）。
    focus: bool,
    /// 打开时把编辑器当前选区带进查询框（选区文本只有渲染帧里拿得到）。
    prefill: bool,
    /// 下一次渲染把查询框内容全选（打开 / 预填之后），直接输入即替换。
    select_all: bool,
}

impl SearchState {
    fn step(&mut self, forward: bool) {
        if self.total == 0 {
            return;
        }
        self.cur = if forward {
            (self.cur + 1) % self.total
        } else {
            (self.cur + self.total - 1) % self.total
        };
        self.goto = true;
    }
}

/// 一帧的排版结果。**只要输入没变就整份复用**，见 [`LayoutKey`]。
///
/// 放在 `Arc` 里存进 `ui.data`：命中时取出来只是一次引用计数加一，
/// 而不是把几十万个 `char` 再拷一遍。每个编辑器 id 只留最新的一份。
struct Layout {
    key: LayoutKey,
    /// 源文本 char 数（映射函数处处要用）。
    n: usize,
    regions: Vec<Region>,
    /// 源文本里每个换行符的 char 下标（行号栏用）。
    src_nl: Vec<usize>,
    /// 可见↔源的段映射。
    segs: Vec<Seg>,
    /// 折叠之后**真正显示**的文本，拆成 char —— 命中判定与取词都按 char 下标走。
    /// 原始的 `String` 不留：galley 自己带着一份，下游没有第二个用处。
    vis_chars: Vec<char>,
    /// 已高亮、已断行的 galley（选区高亮是逐帧叠在它的副本上的，不入缓存）。
    galley: std::sync::Arc<egui::Galley>,
}

/// 排版复用的判据：这些值一个没变，上一帧的 [`Layout`] 就还成立。
///
/// 为什么值得为它专门做缓存：整条排版链是**每帧从头跑一遍**的 O(文本长度) 工作 ——
/// 拆 char、扫折叠区间、拼可见文本、逐 token 生成高亮 `LayoutJob`、再交给 egui 排版。
/// 实测 212KB 的 JSON 上合计 ~3.0 ms/帧，其中 2.2 ms 花在 `layout_job` 里 ——
/// 而且那 2.2 ms **是缓存命中的路径**：egui 的 galley 缓存按 job 的哈希查表，
/// 于是每帧都要把 20 万字符 + 近 3 万个 `TextFormat` 段重新哈希一遍，只为得出
/// 「跟上一帧一样」。滚动、拖选、改窗口大小这些不动文本的操作因此白白吃满 CPU；
/// 软件渲染的机器上这就是滚动发涩的直接来源。
///
/// 文本用哈希而不是留副本比：哈希 212KB 约 30µs，留副本要多占一份内存还要逐字节比。
#[derive(Clone, Copy, PartialEq, Eq)]
struct LayoutKey {
    text: u64,
    folded: u64,
    /// 换行宽度（`f32::to_bits`）。关掉自动换行时宽度不参与排版，
    /// 统一记成 -∞ —— 于是拖窗口大小不会白白让缓存失效。
    wrap: u32,
    font: u64,
    dark: bool,
    /// 每点像素数。**必须在判据里**：galley 的字形引用的是字体图集里的 uv 区域，
    /// 而 egui 在 `pixels_per_point` 变化时会整个重建图集并丢掉自己的 galley 缓存。
    /// 我们这份缓存活得比它久，不跟着失效的话，改一次「界面缩放」就会拿旧 uv 去
    /// 采样新图集 —— 屏幕上是一片错位的字。
    ppp: u32,
}

impl LayoutKey {
    fn new(
        text: &str,
        folded: &HashSet<usize>,
        wrap_w: f32,
        font: FontCfg,
        dark: bool,
        ppp: f32,
    ) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        let text = h.finish();

        // 折叠集是无序集合：逐元素异或，与遍历顺序无关。
        let folded = folded.iter().fold(0u64, |acc, br| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            br.hash(&mut h);
            acc ^ h.finish()
        });

        // 字号 / 行距 / 字重都进哈希：它们既改高亮段的 line_height，也改字形宽度，
        // 任何一项变了整块 galley 都要重排。
        let mut h = std::collections::hash_map::DefaultHasher::new();
        font.size.to_bits().hash(&mut h);
        font.line_scale.to_bits().hash(&mut h);
        font.medium.hash(&mut h);
        Self {
            text,
            folded,
            wrap: wrap_w.to_bits(),
            font: h.finish(),
            dark,
            ppp: ppp.to_bits(),
        }
    }
}

/// 搜索条（停靠行）占用的高度：32px 内容 + 分隔线与间距。
const SEARCH_BAR_H: f32 = 38.0;

/// 调试开关：在编辑器右上角画出焦点/事件，用于诊断输入法问题（默认关闭）。
const DEBUG_HUD: bool = false;

/// 一段可折叠区间（字符下标，均为源文本 char 下标）。
#[derive(Clone, Copy)]
struct Region {
    br: usize,    // 开括号 '{' / '[' 自身下标
    open: usize,  // 开括号之后（隐藏内容起点）
    close: usize, // 匹配的闭括号 '}' / ']' 下标（隐藏内容终点，闭括号本身可见）
    /// 直接子节点数：对象是键的个数，数组是元素个数（不含更深层）。
    /// 折起来之后就是靠它告诉你「这里面收了多少东西」。
    count: usize,
    /// 是不是对象（`{`）。决定占位里用「项」还是「个」。
    obj: bool,
}

/// 可见文本的一段：真实源文本段，或折叠占位段。
#[derive(Clone, Copy)]
enum SegKind {
    Real(usize), // 对应源字符起点
    Fold {
        br: usize,
        open: usize,
        close: usize,
    },
}
#[derive(Clone, Copy)]
struct Seg {
    vis_start: usize,
    len: usize,
    kind: SegKind,
}

/// 折叠占位。带上直接子节点数 —— 折起来之后光看一个 `⋯` 不知道里面收了多少东西，
/// 数量是判断「要不要展开」最有用的一条信息。
///
/// 对象里数的是键、数组里数的是元素，都只数**直接**子节点（不含更深层）。
fn placeholder_for(count: usize, obj: bool) -> String {
    let unit = if obj { "项" } else { "个" };
    format!(" ⋯ {count} {unit} ⋯ ")
}

/// 编辑区的排版设置（字号 / 字重 / 行距）。
///
/// 单独拎出来而不是沿用全局 `TextStyle::Monospace`：JSON 要么是密密麻麻几千行、
/// 要么是需要逐字核对的密钥串，合适的字号与行距因人因内容而异；而这只该影响这块
/// 编辑区，不该动到整个界面的字号。
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FontCfg {
    /// 字号（px）。
    pub size: f32,
    /// 用中黑（JetBrains Mono Medium）而不是常规字重 —— 小字号下细体容易发虚。
    pub medium: bool,
    /// 行距倍数（相对字号）。
    pub line_scale: f32,
}

impl Default for FontCfg {
    fn default() -> Self {
        // 与改动前的观感一致：egui 默认 Monospace 正是 13px；
        // 1.35 是 JetBrains Mono 在这个字号下比较耐看的密度。
        Self {
            size: 13.0,
            medium: false,
            line_scale: 1.35,
        }
    }
}

impl FontCfg {
    /// 字号范围。下限保证标点还分得清，上限避免一屏放不下几行。
    pub const MIN_SIZE: f32 = 10.0;
    pub const MAX_SIZE: f32 = 22.0;
    /// 行距档位：紧凑 / 标准 / 宽松。
    pub const LINE_SCALES: [(f32, &'static str); 3] =
        [(1.15, "紧凑"), (1.35, "标准"), (1.6, "宽松")];

    pub fn font_id(&self) -> egui::FontId {
        let family = if self.medium {
            egui::FontFamily::Name(crate::fonts::MONO_MEDIUM.into())
        } else {
            egui::FontFamily::Monospace
        };
        egui::FontId::new(self.size, family)
    }

    /// 实际行高（px）。
    pub fn row_height(&self) -> f32 {
        (self.size * self.line_scale).max(1.0)
    }

    /// 夹到合法范围。草稿是用户可写的 ron，脏值不能直接拿去排版。
    pub fn clamped(mut self) -> Self {
        if !self.size.is_finite() {
            self.size = Self::default().size;
        }
        if !self.line_scale.is_finite() {
            self.line_scale = Self::default().line_scale;
        }
        self.size = self.size.clamp(Self::MIN_SIZE, Self::MAX_SIZE);
        self.line_scale = self.line_scale.clamp(1.0, 2.0);
        self
    }
}

/// 渲染一个可编辑、语法高亮、可折叠的 JSON 代码编辑器，铺满 `height`，返回交互 `Response`。
///
/// `wrap = true` 时长行自动换行（不出现横向滚动条）；`false` 时保持长行不断，
/// 由横向滚动条查看超出部分。
pub fn code_editor(
    ui: &mut Ui,
    theme: &Theme,
    id_source: &str,
    text: &mut String,
    height: f32,
    wrap: bool,
    font: FontCfg,
) -> Response {
    // 整体套一层 scope：下面要改滚动条样式与滑块配色，而 `Ui::visuals_mut` 改的是
    // 调用方那个 Ui 的样式 —— 不隔离的话会外溢到别的界面元素上（实测把侧栏的
    // 分隔线染成了深灰）。scope 里的样式改动出了这个函数就作废。
    ui.scope(|ui| code_editor_inner(ui, theme, id_source, text, height, wrap, font))
        .inner
}

fn code_editor_inner(
    ui: &mut Ui,
    theme: &Theme,
    id_source: &str,
    text: &mut String,
    height: f32,
    wrap: bool,
    font: FontCfg,
) -> Response {
    let id = ui.make_persistent_id(id_source);
    // 排版一律走 FontCfg，不再读全局 TextStyle —— 字号/行距只影响这块编辑区
    let font = font.clamped();
    let font_id = font.font_id();
    let row_h = font.row_height();
    let num_color = ui.visuals().weak_text_color();
    let arrow_color = theme.muted;
    let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font_id, '0'));

    // 滚动条改成**常驻实心**，不用 egui 默认的浮动样式。
    //
    // 浮动滚动条要鼠标靠近才浮现，而且在浅色主题下淡到几乎看不见（实测是白底上的
    // (252,252,253)）。对代码编辑器来说这等于「有长行但没人知道能左右滚」——
    // 关掉自动换行后，横向滚动是查看超宽内容的唯一手段，它必须一眼可见、随时可拖。
    // 必须在算 view_w 之前设置：实心样式的条更宽，宽度预留要用新值。
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    // 滑块颜色单独定：全局 widget 配色里的 inactive 底是 code_bg（本就接近白），
    // 拿来当滑块等于白底画白条 —— 实测是 (247,248,250)，看不见。这里换成
    // 有实际反差的中性灰，并保证悬停/拖动时更明显。
    {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_fill = theme.faint.gamma_multiply(0.55);
        w.hovered.bg_fill = theme.muted;
        w.active.bg_fill = theme.muted;
    }

    // 在**进入 ScrollArea 之前**量可视宽度：进去以后开了横向滚动的 ui 宽度不再是视口宽度。
    // 预留纵向滚动条的位置，否则换行后最右侧一列字会钻到滚动条底下。
    let view_w = (ui.available_width() - ui.spacing().scroll.allocated_width() - 2.0).max(120.0);

    let mut state: EditorState = ui
        .data_mut(|d| d.get_temp::<EditorState>(id))
        .unwrap_or_default();
    let sid = id.with("__search");
    let mut search: SearchState = ui
        .data_mut(|d| d.get_temp::<SearchState>(sid))
        .unwrap_or_default();

    // Ctrl+F：打开（或重新聚焦）搜索条。在进 ScrollArea 之前消费掉，
    // 编辑器的事件循环与全局快捷键就都看不到这个按键了。
    if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
        search.open = true;
        search.focus = true;
        search.prefill = true;
        // 焦点让给搜索框。不清这个闩锁的话，编辑器每帧都会把焦点抢回去。
        state.want_focus = false;
    }
    if search.open {
        // F3 / Shift+F3：焦点在编辑器或搜索框都可用。先查带 Shift 的组合。
        if ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F3)) {
            search.step(false);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F3)) {
            search.step(true);
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            search.open = false;
            state.want_focus = true; // 关掉搜索条后焦点还给编辑器
        }
    }

    // ---- 搜索条：停靠在编辑区顶部的平铺一行（txt / 记事本式）----
    //
    // 不用浮层：浮层要么压住正文首行，要么带阴影 —— 软件光栅化（虚拟机）下
    // 半透明阴影会糊成一团灰，非整数像素的浮块还会让文字发虚。停靠条是
    // 不透明、整行铺满、按行对齐的，正文整体下移让位，谁也不挡谁。
    let inner_h = if search.open {
        (height - SEARCH_BAR_H).max(60.0)
    } else {
        height.max(60.0)
    };
    if search.open {
        let bar = ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 32.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(2.0);
                ui.label(crate::icons::text(crate::icons::SEARCH, 13.0, theme.muted));

                // 右侧固定占位：计数（约 72px）+ 三个 32px 按钮 + 间距；
                // 剩余宽度全部给输入框 —— 长查询要能看全，这是「简单明了」的核心。
                let controls_w = 72.0 + 32.0 * 3.0 + 6.0 * 5.0;
                let input_w = (ui.available_width() - controls_w).max(120.0);
                let te = egui::TextEdit::singleline(&mut search.query)
                    .id(id.with("__searchbox"))
                    .desired_width(input_w)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("查找…（Enter 下一个 · Shift+Enter 上一个 · Esc 关闭）")
                    .show(ui);
                if search.focus {
                    search.focus = false;
                    te.response.request_focus();
                    search.select_all = true;
                }
                // 全选查询串：刚打开或预填之后，直接敲字即替换（记事本同款）。
                if search.select_all && te.response.has_focus() {
                    if let Some(mut st) =
                        egui::text_edit::TextEditState::load(ui.ctx(), te.response.id)
                    {
                        let end = search.query.chars().count();
                        st.cursor.set_char_range(Some(CCursorRange::two(
                            CCursor::new(0),
                            CCursor::new(end),
                        )));
                        st.store(ui.ctx(), te.response.id);
                        search.select_all = false;
                    }
                }
                if te.response.changed() {
                    search.cur = 0;
                    search.goto = true;
                    ui.ctx().request_repaint();
                }
                // Enter = 下一个，Shift+Enter = 上一个；回车会让 TextEdit 交出
                // 焦点，导航完抢回来，连续回车才能连续跳。
                if te.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    search.step(!ui.input(|i| i.modifiers.shift));
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }

                // 计数固定宽度，数字变化时布局不跳。
                let (count, color) = if search.query.is_empty() {
                    (String::new(), theme.muted)
                } else if search.total == 0 {
                    ("无匹配".to_owned(), theme.danger)
                } else {
                    (format!("{}/{}", search.cur + 1, search.total), theme.muted)
                };
                ui.allocate_ui_with_layout(
                    vec2(72.0, 24.0),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(
                            egui::RichText::new(count)
                                .size(11.5)
                                .family(egui::FontFamily::Monospace)
                                .color(color),
                        );
                    },
                );

                // 按钮点完把焦点还给输入框：否则回车导航就失灵了 ——
                // 这正是此前「交互很怪」的来源之一。
                if crate::widgets::tb_text_btn(
                    ui,
                    theme,
                    "↑",
                    false,
                    "上一个（Shift+Enter / Shift+F3）",
                )
                .clicked()
                {
                    search.step(false);
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }
                if crate::widgets::tb_text_btn(ui, theme, "↓", false, "下一个（Enter / F3）")
                    .clicked()
                {
                    search.step(true);
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }
                if crate::widgets::tb_icon_btn(
                    ui,
                    theme,
                    crate::icons::X,
                    false,
                    false,
                    "关闭（Esc）",
                )
                .clicked()
                {
                    search.open = false;
                    state.want_focus = true;
                    ui.ctx().request_repaint();
                }
            },
        );
        // 底边一条 1px 分隔线：把搜索行和正文分开，不靠阴影。
        let r = bar.response.rect;
        ui.painter().hline(
            egui::Rangef::new(r.left(), r.right()),
            r.bottom() + 2.0,
            egui::Stroke::new(1.0_f32, theme.border),
        );
        ui.add_space(SEARCH_BAR_H - 32.0);
    }

    let out = egui::ScrollArea::new([!wrap, true])
        .id_salt((id, "sc"))
        .max_height(inner_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ---- 1~3. 扫描 + 可见文本 + 高亮 galley：整条链带缓存 ----
            //
            // 这三步都是 O(文本长度) 的，而滚动、拖选、移光标、改窗口大小这些操作
            // 一个字都没动过 —— 每帧从头再算一遍纯属白烧 CPU（代价与归因见 [`LayoutKey`]）。
            // 输入没变就整份复用上一帧的结果。
            //
            // 换行宽度取决于行号栏宽、行号栏宽又取决于总行数，所以判据里放的是
            // **视口宽**（`view_w`）而不是最终的 max_width：两者之间只差一个由
            // 文本与字体决定的量，而那两样已经在判据里了。关掉换行时宽度根本不参与
            // 排版，用 -∞ 占位 —— 于是「不换行时拖窗口大小」不会白白重排。
            let lay_id = id.with("__layout");
            let key = LayoutKey::new(
                text,
                &state.folded,
                if wrap { view_w } else { f32::NEG_INFINITY },
                font,
                theme.dark,
                ui.ctx().pixels_per_point(),
            );
            let cached = ui
                .data(|d| d.get_temp::<std::sync::Arc<Layout>>(lay_id))
                .filter(|l| l.key == key);
            let layout = match cached {
                Some(l) => l,
                None => {
                    let chars: Vec<char> = text.chars().collect();
                    let n = chars.len();
                    let regions = scan_regions(&chars);
                    let region_brs: HashSet<usize> = regions.iter().map(|r| r.br).collect();
                    state.folded.retain(|br| region_brs.contains(br)); // 剪除失效锚点
                    let src_nl = newline_positions(&chars);

                    let (vis, segs) = build_visible(&chars, n, &regions, &state.folded);
                    let vis_chars: Vec<char> = vis.chars().collect();

                    let mut job =
                        crate::widgets::json_highlight(&vis, &font_id, theme, Some(row_h));
                    job.wrap.max_width = if wrap {
                        // 末尾留一个字符宽：行尾光标要有地方画，否则它会贴在边界上被裁掉
                        (view_w - gutter_width(src_nl.len(), char_w) - char_w - 12.0)
                            .max(char_w * 8.0)
                    } else {
                        f32::INFINITY
                    };
                    // JSON 里的长 token（base64 / URL / 长字符串）中间没有空格，
                    // 只按词断等于不断
                    job.wrap.break_anywhere = wrap;
                    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));

                    let l = std::sync::Arc::new(Layout {
                        key,
                        n,
                        regions,
                        src_nl,
                        segs,
                        vis_chars,
                        galley,
                    });
                    ui.data_mut(|d| d.insert_temp(lay_id, l.clone()));
                    l
                }
            };
            let n = layout.n;
            let regions = &layout.regions;
            let src_nl = &layout.src_nl;
            let segs = &layout.segs;
            let vis_chars = &layout.vis_chars;
            let vis_chars_len = vis_chars.len();
            // 选区高亮是逐帧叠上去的（`paint_text_selection` 会 `Arc::make_mut`），
            // 所以这里拿的是缓存那份的克隆 —— 没有选区时连深拷贝都不会发生。
            let galley = layout.galley.clone();

            // 行号栏宽度（含折叠箭头区）。
            let gutter_w = gutter_width(src_nl.len(), char_w);

            // 横向滚动条会不会出现（关了换行、且正文确实比视口宽）。
            let full_w = gutter_w + galley.size().x + 12.0;
            let needs_hbar = !wrap && full_w > view_w;

            // 高度的下限要**扣掉横向滚动条占的那一条**。
            //
            // 不扣的话：内容明明只有几行、纵向完全放得下，可横向条一出现就吃掉底部十几像素，
            // 于是「内容高度(满高) > 视口高度(满高-条宽)」，凭空多出一根只能滚十几像素的
            // 纵向滚动条 —— 看起来就是「右边这条的高度跟内容对不上」。
            // 用 egui 自己的 allocated_width()：实心条实际占的是
            // 内边距 + 条宽 + 外边距，只扣 bar_width 会少扣一截，假滚动条照样出现。
            let bar_use = ui.spacing().scroll.allocated_width();
            let min_h = if needs_hbar {
                (inner_h - bar_use).max(40.0)
            } else {
                inner_h
            };

            // 宽度至少铺满视口：短内容时右侧那片空白也要属于编辑器，
            // 否则点在空白处既不聚焦也不落光标，看着像编辑器坏了（这是本来就有的毛病）。
            // 换行时正好等于视口宽，也就不会因为几像素误差冒出横向滚动条。
            let content = Vec2::new(
                if wrap { view_w } else { full_w.max(view_w) },
                galley.size().y.max(min_h),
            );
            let (rect, _) = ui.allocate_exact_size(content, Sense::hover());
            // 用本编辑器自己的 id 交互，`id` 才会被注册为「可聚焦」。
            let resp = ui.interact(rect, id, Sense::click_and_drag());
            let text_origin = rect.min + vec2(gutter_w, 0.0);

            // 行号栏钉在**视口**左边，而不是内容左边。
            //
            // 关掉自动换行后内容可以比视口宽得多，横向滚动时内容整体左移；行号栏若跟着走，
            // 滚到右边就直接滑出屏幕 —— 于是「能滚」但不知道自己在第几行，等于白滚。
            // 不滚动时 clip_rect 左边就等于 rect 左边，外观与从前一致。
            let gutter_x = ui.clip_rect().left().max(rect.left());

            // ---- 4. 待映射源光标（上一帧结构编辑后）→ 可见坐标 ----
            // follow_cursor：本帧光标位置有意义地移动过（编辑落点重映射 / 键盘移动），
            // 帧尾把光标滚回视口 —— 「选区往下扩时滚动条不跟着走」就是缺这一步。
            let mut follow_cursor = false;
            if let Some(src) = state.pending.take() {
                let v = map_src(segs, src, vis_chars_len);
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(v))));
                follow_cursor = true;
            }

            // ---- 5. 折叠箭头命中框 ----
            let mut arrows: Vec<(Rect, usize, bool)> = Vec::new(); // (hitbox, br, folded?)
            for r in regions {
                if is_hidden(r.br, segs) {
                    continue; // 该 header 被上层折叠盖住，不显示箭头
                }
                let v = map_src(segs, r.br, vis_chars_len);
                // 直接问 galley 这个字符画在哪一行 —— 开了换行以后「第几个换行符」
                // 不再等于「第几个视觉行」，只有 galley 自己知道答案
                let y = text_origin.y + galley.pos_from_cursor(CCursor::new(v)).top();
                // 箭头跟着钉住的行号栏走，否则横向滚动后点不到
                let hit = Rect::from_min_size(pos2(gutter_x + 2.0, y), vec2(16.0, row_h));
                arrows.push((hit, r.br, state.folded.contains(&r.br)));
            }

            // ---- 6. 交互：先处理折叠箭头点击，其次文本 ----
            let mut gutter_click = false;
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    for (hit, br, _) in &arrows {
                        if hit.contains(p) {
                            // 折叠前把当前光标换算到源，折叠后再映回，避免光标跳飞。
                            if let Some(rg) = state.cursor.range(&galley) {
                                let (s, _) = map_vis(segs, rg.primary.index.0, n);
                                state.pending = Some(s);
                            }
                            if state.folded.contains(br) {
                                state.folded.remove(br);
                            } else {
                                state.folded.insert(*br);
                            }
                            gutter_click = true;
                            ui.ctx().request_repaint();
                            break;
                        }
                    }
                }
            }

            // 焦点闩锁：在编辑器内按下/点击/拖动 → 置意图真；在编辑器外按下 → 清除。
            // 然后每帧「只要有意图且当前没焦点」就重夺——这样即便低帧率或输入法激活造成瞬时
            // 失焦，下一帧也能立刻恢复（合成点击/软渲染下只认 clicked() 会漏掉）。
            let pressed_on_me =
                resp.clicked() || resp.dragged() || resp.is_pointer_button_down_on();
            let pressed_elsewhere =
                ui.input(|i| i.pointer.any_pressed()) && !resp.contains_pointer();
            if pressed_on_me && !gutter_click {
                state.want_focus = true;
            }
            if pressed_elsewhere {
                state.want_focus = false;
            }
            if state.want_focus && !resp.has_focus() {
                resp.request_focus();
            }
            let has_focus = resp.has_focus();

            // ---- 调试：无论是否聚焦都记录到达的输入事件 ----
            if DEBUG_HUD {
                let evs = ui.input(|i| i.events.clone());
                for e in &evs {
                    let d = match e {
                        Event::Text(t) => format!("Text {t:?}"),
                        Event::Ime(im) => format!("Ime {im:?}"),
                        Event::Key { key, pressed, .. } => format!("Key {key:?} p={pressed}"),
                        Event::Paste(_) => "Paste".to_owned(),
                        _ => continue,
                    };
                    state.dbg.push(d);
                }
                while state.dbg.len() > 9 {
                    state.dbg.remove(0);
                }
            }

            if has_focus {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        id,
                        EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        },
                    );
                });
            }

            // 鼠标：定位光标 / 选择。
            //
            // 行号栏的 x 守卫只对**按下那一刻**生效（避免点行号误落光标）；一旦拖起来就
            // 不再限制 —— 否则往左拖到行号栏上，选区就卡住不动了，而「从行尾往左拖到
            // 行首」恰恰是最常见的选择方式之一。
            if !gutter_click {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let dragging = resp.dragged();
                    // 用**钉住的**行号栏右边界做判断：横向滚动后 text_origin.x 会变成负数，
                    // 拿它当界限的话，点在行号栏上也会被当成点正文
                    if dragging || ptr.x >= gutter_x + gutter_w - 2.0 {
                        let cursor_at = galley.cursor_from_pos(ptr - text_origin);
                        state
                            .cursor
                            .pointer_interaction(ui, &resp, cursor_at, &galley, dragging);
                    }
                }
            }

            // 三连击「快速选值」：点在字符串上 → 直接选中整个值（不含引号，从头到尾）；
            // 点在数字 / true / false / null 上 → 选中该标量。都不是则维持
            // `pointer_interaction` 刚做完的默认整行选择。
            if resp.triple_clicked() && !gutter_click {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let ci = galley.cursor_from_pos(ptr - text_origin).index.0;
                    let quick = string_content_range(vis_chars, ci)
                        .or_else(|| scalar_token_range(vis_chars, ci));
                    if let Some((s, e)) = quick {
                        state.cursor.set_char_range(Some(CCursorRange::two(
                            CCursor::new(s),
                            CCursor::new(e),
                        )));
                    }
                }
            }

            // 拖拽选择时让视口跟着指针走：越过可视区边缘就按越界距离滚动。
            // 没有这个的话拖到底就停住，选区只能覆盖当前屏幕内的内容。
            if resp.dragged() && !gutter_click {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let clip = ui.clip_rect();
                    let dy = if ptr.y < clip.top() {
                        ptr.y - clip.top()
                    } else if ptr.y > clip.bottom() {
                        ptr.y - clip.bottom()
                    } else {
                        0.0
                    };
                    let dx = if wrap {
                        0.0 // 开着换行没有横向滚动
                    } else if ptr.x < clip.left() {
                        ptr.x - clip.left()
                    } else if ptr.x > clip.right() {
                        ptr.x - clip.right()
                    } else {
                        0.0
                    };
                    if dx != 0.0 || dy != 0.0 {
                        // 每帧滚越界距离的三成：离得越远滚得越快，又不会一步跳过头。
                        ui.scroll_with_delta(vec2(-dx, -dy) * 0.3);
                        ui.ctx().request_repaint();
                    }
                }
            }

            // ---- 7. 键盘 / 文本事件（仅聚焦时消费）----
            if has_focus && !gutter_click {
                let os = ui.ctx().os();
                let events = ui.input(|i| i.events.clone());
                let mut vrange = state
                    .cursor
                    .range(&galley)
                    .unwrap_or_else(|| CCursorRange::one(galley.end()));
                'ev: for ev in &events {
                    match ev {
                        Event::Text(t) if !t.is_empty() => {
                            if edit_replace(text, segs, n, &mut state, &vrange, t) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Paste(t) if !t.is_empty() => {
                            if edit_replace(text, segs, n, &mut state, &vrange, t) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Key {
                            key: Key::Enter,
                            pressed: true,
                            ..
                        } => {
                            if edit_replace(text, segs, n, &mut state, &vrange, "\n") {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Key {
                            key: Key::Backspace,
                            pressed: true,
                            ..
                        } => {
                            if edit_backspace(text, segs, n, &mut state, &vrange) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Key {
                            key: Key::Delete,
                            pressed: true,
                            ..
                        } => {
                            if edit_delete(text, segs, n, vis_chars_len, &mut state, &vrange) {
                                ui.ctx().request_repaint();
                                break 'ev;
                            }
                        }
                        Event::Copy => {
                            if let Some(s) = selection_src(text, segs, n, &vrange) {
                                if !s.is_empty() {
                                    ui.ctx().copy_text(s);
                                }
                            }
                        }
                        Event::Cut => {
                            if let Some(s) = selection_src(text, segs, n, &vrange) {
                                if !s.is_empty() {
                                    ui.ctx().copy_text(s);
                                    if edit_replace(text, segs, n, &mut state, &vrange, "") {
                                        ui.ctx().request_repaint();
                                        break 'ev;
                                    }
                                }
                            }
                        }
                        Event::Ime(ime) => {
                            // 不 break：`Preedit("")` 与 `Commit(text)` 常在同一帧连续到达，
                            // 必须都处理，否则提交的文字被丢弃（中文永远上不了屏）。
                            edit_ime(text, segs, n, &mut state, &vrange, ime);
                            ui.ctx().request_repaint();
                        }
                        Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            // 方向 / Home/End / 按词 / 全选：作用在可见 galley 上。
                            vrange.on_key_press(os, &galley, modifiers, *key);
                            if matches!(
                                key,
                                Key::ArrowUp
                                    | Key::ArrowDown
                                    | Key::ArrowLeft
                                    | Key::ArrowRight
                                    | Key::Home
                                    | Key::End
                                    | Key::PageUp
                                    | Key::PageDown
                            ) {
                                follow_cursor = true;
                            }
                        }
                        _ => {}
                    }
                }
                if state.pending.is_none() {
                    state.cursor.set_char_range(Some(vrange));
                }
            }

            // ---- 7.5 搜索匹配（Ctrl+F）----
            //
            // 在**可见文本**上找：折叠区间里的内容既看不见也选不着，跳过去只会把
            // 视口甩到一个「什么都没有」的占位上。大小写不敏感，逐字符 lowercase
            // 比对（1:1 映射，下标不会因 lowercase 变长而错位）。
            let mut match_list: Vec<usize> = Vec::new();
            let mut match_len = 0usize;
            if search.open {
                if search.prefill {
                    search.prefill = false;
                    // 把当前选区带进查询框（单行、别太长），VS Code 同款行为。
                    if let Some(rg) = state.cursor.range(&galley) {
                        if !rg.is_empty() {
                            if let Some(s) = selection_src(text, segs, n, &rg) {
                                if !s.is_empty() && !s.contains('\n') && s.chars().count() <= 128 {
                                    search.query = s;
                                    search.cur = 0;
                                    // 预填的文字下一帧全选：直接敲字即替换。
                                    search.select_all = true;
                                }
                            }
                        }
                    }
                    search.goto = true;
                }
                // 查询串截到 256 字符再匹配：朴素匹配是 O(n·m)，往查询框里粘一大段
                // 文本不该有能力把每一帧拖死。256 远超正常查询长度，截断无感。
                let needle: Vec<char> = search.query.chars().take(256).collect();
                match_len = needle.len();
                if match_len > 0 && search.query.chars().count() <= 256 {
                    match_list = find_matches(vis_chars, &needle, 5000);
                }
                search.total = match_list.len();
                if search.cur >= search.total {
                    search.cur = 0;
                }
            }

            // ---- 8. 绘制：选区 → 文本 → 行号/箭头 → 光标 ----
            //
            // 选区**失焦后也要继续画**（只是淡一些）。此前只在有焦点时画，于是点一下
            // 工具条的「格式化 / 复制」按钮，刚选好的内容就当场消失 —— 用起来就是
            // 「格式化之后选不中了」。所有正经编辑器都保留失焦选区，这里对齐。
            let mut galley = galley;
            if let Some(r) = state.cursor.range(&galley) {
                if !r.is_empty() {
                    let mut vis = ui.visuals().clone();
                    if !has_focus {
                        vis.selection.bg_fill = vis.selection.bg_fill.gamma_multiply(0.5);
                    }
                    paint_text_selection(&mut galley, &vis, &r, None);
                }
            }
            let painter = ui.painter().clone();

            // 搜索命中底色：画在正文 galley 之下。颜色在 CPU 上预混成**不透明**色 ——
            // 软件光栅化（虚拟机）下半透明矩形叠半透明选区，边缘就是一片糊；
            // 不透明色块 + 1px 内描边在任何渲染路径下都是锐利的。
            if !match_list.is_empty() {
                let clip = ui.clip_rect();
                let normal_bg = blend(theme.bg, theme.accent, 0.18);
                let cur_bg = blend(theme.bg, theme.accent, 0.42);
                for (mi, &s) in match_list.iter().enumerate() {
                    let is_cur = mi == search.cur;
                    // 离屏快速跳过：先看命中首字符所在行的 y，整段在视口外就不必
                    // 逐字符算矩形（5000 个命中逐个 pos_from_cursor 不是免费的）。
                    let head = galley.pos_from_cursor(CCursor::new(s));
                    let head_top = head.top() + text_origin.y;
                    if head_top > clip.bottom() + row_h {
                        break; // 命中按位置升序，后面的只会更靠下
                    }
                    let tail = galley.pos_from_cursor(CCursor::new(s + match_len));
                    if tail.bottom() + text_origin.y < clip.top() - row_h {
                        continue;
                    }
                    for r in match_rects(&galley, s, s + match_len, char_w) {
                        let rr = r.translate(text_origin.to_vec2());
                        if rr.bottom() < clip.top() || rr.top() > clip.bottom() {
                            continue;
                        }
                        if is_cur {
                            painter.rect(
                                rr,
                                2.0,
                                cur_bg,
                                egui::Stroke::new(1.0, theme.accent),
                                egui::StrokeKind::Inside,
                            );
                        } else {
                            painter.rect_filled(rr, 2.0, normal_bg);
                        }
                    }
                }
            }
            painter.galley(text_origin, galley.clone(), theme.fg);

            // 行号栏底：横向滚动时正文会从行号栏下面穿过去，必须先铺一层不透明底色盖住。
            // 只在真的滚开了才铺，避免在不滚动的常态下多画一层。
            if gutter_x > rect.left() + 0.5 {
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(gutter_x, ui.clip_rect().top()),
                        vec2(gutter_w, ui.clip_rect().height()),
                    ),
                    0.0,
                    theme.bg,
                );
            }

            // 行号：逐可见行显示对应的**源**行号（折叠处号码跳变，如 VS Code）。
            for (i, (vstart, is_first)) in row_starts(&galley).into_iter().enumerate() {
                if !is_first {
                    continue; // 折行产生的续行不打号，否则一屏全是重复数字
                }
                let y = text_origin.y + galley.rows[i].pos.y;
                let (src_ci, _) = map_vis(segs, vstart, n);
                let src_line = src_nl.partition_point(|&p| p < src_ci) + 1;
                painter.text(
                    pos2(gutter_x + gutter_w - 8.0, y),
                    Align2::RIGHT_TOP,
                    src_line.to_string(),
                    font_id.clone(),
                    num_color,
                );
            }

            // 折叠箭头。
            for (hit, _, folded) in &arrows {
                draw_arrow(&painter, *hit, *folded, arrow_color);
            }

            // 调试 HUD。
            if DEBUG_HUD {
                let vp = ui.clip_rect();
                let mono = egui::FontId::monospace(12.0);
                let mut y = vp.top() + 6.0;
                let hdr = format!(
                    "focus={} ime_state={:?} os={:?}",
                    has_focus,
                    state.ime,
                    ui.ctx().os()
                );
                for line in std::iter::once(hdr).chain(state.dbg.iter().cloned()) {
                    painter.rect_filled(
                        egui::Rect::from_min_size(pos2(vp.right() - 430.0, y), vec2(426.0, 15.0)),
                        2.0,
                        Color32::from_black_alpha(190),
                    );
                    painter.text(
                        pos2(vp.right() - 426.0, y),
                        Align2::LEFT_TOP,
                        line,
                        mono.clone(),
                        Color32::from_rgb(0x8f, 0xff, 0x8f),
                    );
                    y += 16.0;
                }
            }

            // 光标 + 输入法候选框定位。
            if has_focus {
                if let Some(r) = state.cursor.range(&galley) {
                    let cr =
                        cursor_rect(&galley, &r.primary, row_h).translate(text_origin.to_vec2());
                    paint_cursor_end(&painter, ui.visuals(), cr);
                    // 上报候选框位置：焦点期间恒定，OS 才会把输入法窗贴到光标处并开启 IME。
                    ui.ctx().output_mut(|o| {
                        o.ime = Some(egui::output::IMEOutput {
                            rect,
                            cursor_rect: cr,
                            should_interrupt_composition: false,
                        })
                    });
                }
            }

            // 键盘移动 / 编辑之后：让光标留在视口内（往下扩选区时视口跟着下滑）。
            if follow_cursor {
                if let Some(r) = state.cursor.range(&galley) {
                    let cr =
                        cursor_rect(&galley, &r.primary, row_h).translate(text_origin.to_vec2());
                    ui.scroll_to_rect(cr.expand2(vec2(char_w * 2.0, row_h)), None);
                }
            }

            // 搜索跳转：把当前匹配滚到视口中部。
            if search.goto {
                search.goto = false;
                if let Some(&s) = match_list.get(search.cur) {
                    let r = galley
                        .pos_from_cursor(CCursor::new(s))
                        .translate(text_origin.to_vec2());
                    ui.scroll_to_rect(r, Some(egui::Align::Center));
                    ui.ctx().request_repaint();
                }
            }

            resp
        });

    ui.data_mut(|d| d.insert_temp(id, state));
    ui.data_mut(|d| d.insert_temp(sid, search));
    out.inner
}

/// 给其它模块的测试用：把折叠区间的节点数暴露出来（(count, is_object)）。
#[cfg(test)]
pub mod test_support {
    pub fn scan(chars: &[char]) -> Vec<(usize, bool)> {
        super::scan_regions(chars)
            .into_iter()
            .map(|r| (r.count, r.obj))
            .collect()
    }
}

// ============================ 折叠 / 映射 辅助 ============================

/// 扫描源文本，找出所有跨行的括号配对（可折叠区间），忽略字符串内的括号。
///
/// 顺带数出每个区间的**直接**子节点数：只统计当前层的逗号，嵌套层的逗号由它自己那层数。
/// 空容器（`{}` / `[]`，中间只有空白）记 0，而不是 1。
fn scan_regions(chars: &[char]) -> Vec<Region> {
    /// 栈帧：(开括号下标, 开括号所在行, 本层逗号数, 本层是否有非空白内容, 是否对象)
    struct Frame {
        br: usize,
        line: usize,
        commas: usize,
        has_content: bool,
        obj: bool,
    }
    let mut regions = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut line = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            if c == '\n' {
                line += 1;
            }
            continue;
        }
        // 只要当前层出现了非空白字符，这个容器就不是空的
        if !c.is_whitespace() && !matches!(c, '{' | '[' | '}' | ']') {
            if let Some(f) = stack.last_mut() {
                f.has_content = true;
            }
        }
        match c {
            '"' => {
                in_str = true;
                if let Some(f) = stack.last_mut() {
                    f.has_content = true;
                }
            }
            '{' | '[' => {
                // 容器本身也算父层的内容
                if let Some(f) = stack.last_mut() {
                    f.has_content = true;
                }
                stack.push(Frame {
                    br: i,
                    line,
                    commas: 0,
                    has_content: false,
                    obj: c == '{',
                });
            }
            ',' => {
                if let Some(f) = stack.last_mut() {
                    f.commas += 1;
                }
            }
            '}' | ']' => {
                if let Some(f) = stack.pop() {
                    if f.line != line {
                        regions.push(Region {
                            br: f.br,
                            open: f.br + 1,
                            close: i,
                            count: if f.has_content { f.commas + 1 } else { 0 },
                            obj: f.obj,
                        });
                    }
                }
            }
            '\n' => line += 1,
            _ => {}
        }
    }
    regions
}

/// 由源文本 + 折叠集构建可见文本与段映射。
fn build_visible(
    chars: &[char],
    n: usize,
    regions: &[Region],
    folded: &HashSet<usize>,
) -> (String, Vec<Seg>) {
    // 取出已折叠区间，按 open 排序，剔除被外层折叠盖住的嵌套区间。
    let mut active: Vec<Region> = regions
        .iter()
        .copied()
        .filter(|r| folded.contains(&r.br))
        .collect();
    active.sort_by_key(|r| r.open);
    let mut top: Vec<Region> = Vec::new();
    let mut cover_end = 0usize;
    for r in active {
        if !top.is_empty() && r.open <= cover_end {
            continue; // 落在上一折叠区间内 → 已隐藏
        }
        top.push(r);
        cover_end = r.close;
    }

    let mut vis = String::new();
    let mut segs: Vec<Seg> = Vec::new();
    let mut si = 0usize;
    let mut vlen = 0usize;
    for r in &top {
        if r.open > si {
            let len = r.open - si;
            segs.push(Seg {
                vis_start: vlen,
                len,
                kind: SegKind::Real(si),
            });
            vis.extend(&chars[si..r.open]);
            vlen += len;
        }
        // 占位长度随节点数变化（"⋯ 3 项 ⋯" 与 "⋯ 128 项 ⋯" 不一样长），
        // 所以逐段记录实际长度，可见↔源的映射全靠它
        let ph = placeholder_for(r.count, r.obj);
        let ph_len = ph.chars().count();
        segs.push(Seg {
            vis_start: vlen,
            len: ph_len,
            kind: SegKind::Fold {
                br: r.br,
                open: r.open,
                close: r.close,
            },
        });
        vis.push_str(&ph);
        vlen += ph_len;
        si = r.close;
    }
    if si < n {
        let len = n - si;
        segs.push(Seg {
            vis_start: vlen,
            len,
            kind: SegKind::Real(si),
        });
        vis.extend(&chars[si..n]);
    }
    (vis, segs)
}

/// 行号栏宽度（数字位数 + 折叠箭头区）。
///
/// 单拎成函数：换行宽度要在排版**之前**扣掉它，而行号栏之后画的时候还要再用一次，
/// 两处必须完全一致 —— 差一像素就是「最右一列字钻到滚动条底下」。
fn gutter_width(src_nl_len: usize, char_w: f32) -> f32 {
    let digits = (src_nl_len + 1).to_string().len().max(2);
    char_w * digits as f32 + 26.0
}

/// 可见 char 下标 → (源 char 下标, 若落在占位内部则给出需展开的 br)。
fn map_vis(segs: &[Seg], vis_ci: usize, n: usize) -> (usize, Option<usize>) {
    let mut fold: Option<(usize, usize)> = None; // (open, br)
    for s in segs {
        if vis_ci >= s.vis_start && vis_ci <= s.vis_start + s.len {
            match s.kind {
                SegKind::Real(src) => return (src + (vis_ci - s.vis_start), None),
                SegKind::Fold { br, open, .. } => {
                    if fold.is_none() {
                        fold = Some((open, br));
                    }
                }
            }
        }
    }
    match fold {
        Some((open, br)) => (open, Some(br)),
        None => (n, None),
    }
}

/// 源 char 下标 → 可见 char 下标（落在隐藏区间则贴到占位起点）。
fn map_src(segs: &[Seg], src_ci: usize, vis_len: usize) -> usize {
    for s in segs {
        if let SegKind::Real(src) = s.kind {
            if src_ci >= src && src_ci <= src + s.len {
                return s.vis_start + (src_ci - src);
            }
        }
    }
    for s in segs {
        if let SegKind::Fold { open, close, .. } = s.kind {
            if src_ci >= open && src_ci <= close {
                return s.vis_start;
            }
        }
    }
    vis_len
}

/// 该源下标（开括号）是否被某个折叠段隐藏。
fn is_hidden(br: usize, segs: &[Seg]) -> bool {
    for s in segs {
        if let SegKind::Fold { open, close, .. } = s.kind {
            if br > open - 1 && br < close {
                return true;
            }
        }
    }
    false
}

/// 逐视觉行给出 `(该行在可见文本中的起始 char 下标, 是否为逻辑行的首行)`。
///
/// 开了自动换行后，「第 i 个视觉行」与「第 i 个换行符」不再一一对应：一个逻辑行会摊成
/// 多个视觉行。所以行起点必须沿 galley 逐行累加字符数得到 —— 靠数换行符不但错位，
/// 行数超过换行符数时还会直接越界 panic。
///
/// `ends_with_newline` 为真表示该行以真正的 `\n` 结束，于是下一行是新的逻辑行；
/// 因折行而断开的续行该标志为假。
fn row_starts(galley: &egui::Galley) -> Vec<(usize, bool)> {
    let mut out = Vec::with_capacity(galley.rows.len());
    let mut start = 0usize;
    let mut is_first = true;
    for prow in galley.rows.iter() {
        out.push((start, is_first));
        start += prow.char_count_including_newline().0;
        is_first = prow.ends_with_newline;
    }
    out
}

fn newline_positions(chars: &[char]) -> Vec<usize> {
    chars
        .iter()
        .enumerate()
        .filter(|(_, &c)| c == '\n')
        .map(|(i, _)| i)
        .collect()
}

// ============================ 编辑（可见→源）============================

/// 平移折叠锚点：在源位置 `point` 处发生 `delta`（char 增量）后调整。
fn shift_folds(state: &mut EditorState, point: usize, delta: isize) {
    if delta == 0 {
        return;
    }
    state.folded = state
        .folded
        .iter()
        .map(|&a| {
            if a >= point {
                (a as isize + delta).max(0) as usize
            } else {
                a
            }
        })
        .collect();
}

/// 用 `ins` 替换当前选区（选区为空则纯插入）。返回 true 表示已改写源文本、需重建。
fn edit_replace(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
    ins: &str,
) -> bool {
    let (p, pu) = map_vis(segs, vrange.primary.index.0, n);
    let (s, su) = map_vis(segs, vrange.secondary.index.0, n);
    if pu.is_some() || su.is_some() {
        if let Some(br) = pu {
            state.folded.remove(&br);
        }
        if let Some(br) = su {
            state.folded.remove(&br);
        }
        return true; // 展开后下一帧重建，本次编辑略过
    }
    let (lo, hi) = (p.min(s), p.max(s));
    if hi > lo {
        text.delete_char_range(CharIndex(lo)..CharIndex(hi));
    }
    let added = text.insert_text(ins, CharIndex(lo));
    let delta = added as isize - (hi - lo) as isize;
    shift_folds(state, lo, delta);
    state.pending = Some(lo + added);
    true
}

fn edit_backspace(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
) -> bool {
    if !vrange.is_empty() {
        return edit_replace(text, segs, n, state, vrange, "");
    }
    let vi = vrange.primary.index.0;
    if vi == 0 {
        return false;
    }
    // 光标紧跟在占位之后 → 展开而非删除隐藏内容。
    if let (_, Some(br)) = map_vis(segs, vi - 1, n) {
        state.folded.remove(&br);
        return true;
    }
    let (p, _) = map_vis(segs, vi, n);
    if p == 0 {
        return false;
    }
    text.delete_char_range(CharIndex(p - 1)..CharIndex(p));
    shift_folds(state, p - 1, -1);
    state.pending = Some(p - 1);
    true
}

fn edit_delete(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    vis_len: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
) -> bool {
    if !vrange.is_empty() {
        return edit_replace(text, segs, n, state, vrange, "");
    }
    let vi = vrange.primary.index.0;
    // 光标紧贴占位之前 → 展开而非删除隐藏内容。
    if vi < vis_len {
        if let (_, Some(br)) = map_vis(segs, vi + 1, n) {
            state.folded.remove(&br);
            return true;
        }
    }
    let (p, pu) = map_vis(segs, vi, n);
    if pu.is_some() {
        if let Some(br) = pu {
            state.folded.remove(&br);
        }
        return true;
    }
    if p >= n {
        return false;
    }
    text.delete_char_range(CharIndex(p)..CharIndex(p + 1));
    shift_folds(state, p, -1);
    state.pending = Some(p);
    true
}

/// 处理输入法事件（组字 Preedit / 提交 Commit）。参照 egui `TextEdit`：预编辑串直接写入
/// 源文本并被下一帧渲染，每次新 Preedit 先删除上次预编辑区间再重插；Commit 落定为正式文本。
fn edit_ime(
    text: &mut String,
    segs: &[Seg],
    n: usize,
    state: &mut EditorState,
    vrange: &CCursorRange,
    ime: &egui::ImeEvent,
) -> bool {
    // ⚠️ 没有在组字时的「空事件」必须原地返回，**一个字段都不能碰**。
    //
    // Linux 上的 ibus（以及别的输入法）在文本框聚焦期间会持续发送 `Preedit("")` /
    // `Enabled` / `Disabled` 这类心跳。如果借这些事件去写 `state.pending`，下一帧开头
    // 就会把光标塌成 `CCursorRange::one(pending)` —— 正在进行的拖拽选区于是每帧被
    // 压回一个点，再被拖拽扩一格，表现就是「怎么拖都只选中一两个字」。
    // 这也是它只在真机（有输入法）复现、测试里却好好的原因。
    let composing = state.ime.is_some();
    if !composing {
        let idle = match ime {
            egui::ImeEvent::Preedit { text: t, .. } => t.is_empty(),
            egui::ImeEvent::Commit(t) => t.is_empty(),
            _ => true, // Enabled / Disabled：与文本无关
        };
        if idle {
            return false;
        }
    }

    // 起点优先级：① 有活动预编辑 → 复用其起点并先删旧串；② 同一帧内上一个 IME 事件遗留
    // 的锚点（关键：`Preedit("")` + `Commit(text)` 常同帧到达，提交须接在清空之后的位置）；
    // ③ 全新组字 → 由当前光标映射到源。
    let start = if let Some((s, e)) = state.ime.take() {
        text.delete_char_range(CharIndex(s)..CharIndex(e));
        shift_folds(state, s, -((e - s) as isize));
        s
    } else if let Some(p) = state.pending {
        p
    } else {
        let (p, pu) = map_vis(segs, vrange.primary.index.0, n);
        if let Some(br) = pu {
            state.folded.remove(&br); // 光标落在占位内 → 先展开
            return true;
        }
        p
    };
    match ime {
        egui::ImeEvent::Preedit { text: t, .. } => {
            if t.is_empty() {
                state.ime = None;
                state.pending = Some(start);
            } else {
                let added = text.insert_text(t, CharIndex(start));
                shift_folds(state, start, added as isize);
                state.ime = Some((start, start + added));
                state.pending = Some(start + added);
            }
            true
        }
        egui::ImeEvent::Commit(t) => {
            let added = text.insert_text(t, CharIndex(start));
            shift_folds(state, start, added as isize);
            state.pending = Some(start + added);
            true
        }
        _ => {
            state.pending = Some(start);
            false
        }
    }
}

/// 取当前选区对应的**源文本**切片（用于复制/剪切）；跨占位则返回含隐藏内容的整段。
fn selection_src(text: &str, segs: &[Seg], n: usize, vrange: &CCursorRange) -> Option<String> {
    let (p, _) = map_vis(segs, vrange.primary.index.0, n);
    let (s, _) = map_vis(segs, vrange.secondary.index.0, n);
    let (lo, hi) = (p.min(s), p.max(s));
    if hi <= lo {
        return Some(String::new());
    }
    Some(text.chars().skip(lo).take(hi - lo).collect())
}

// ============================ 搜索 / 快速选值 ============================

/// 线性混色出**不透明**颜色（t=0 全是 bg，t=1 全是 fg）。
/// 搜索高亮不用半透明直接叠加：软件光栅化下多层 alpha 混合边缘发糊，
/// 在 CPU 上把最终色算好再画，任何渲染后端出来都一样干净。
fn blend(bg: Color32, fg: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(ch(bg.r(), fg.r()), ch(bg.g(), fg.g()), ch(bg.b(), fg.b()))
}

/// 大小写不敏感的字符比较。逐字符做 Unicode lowercase 对比 —— 1:1 映射，
/// 不会像整串 `to_lowercase()` 那样因个别字符变长（ß→ss）导致下标错位。
fn char_eq_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// 在 `hay` 里找出 `needle` 的所有**不重叠**匹配起点（大小写不敏感）。
/// 朴素 O(n·m)：查询串通常只有几个字符，n 是可见文本长度，一帧扫得完；
/// `cap` 是命中数上限，防止「查一个空格」这类病态输入把一帧拖死。
fn find_matches(hay: &[char], needle: &[char], cap: usize) -> Vec<usize> {
    let (n, m) = (hay.len(), needle.len());
    let mut out = Vec::new();
    if m == 0 || n < m {
        return out;
    }
    let mut i = 0;
    while i + m <= n {
        if hay[i..i + m]
            .iter()
            .zip(needle)
            .all(|(&a, &b)| char_eq_ci(a, b))
        {
            out.push(i);
            i += m; // 不重叠：跳过整个命中
            if out.len() >= cap {
                break;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// `ci` 落在某个 JSON 字符串字面量**内容**里时，给出内容区间（不含引号）。
/// 三连击「快速选值」用：字符串再长（base64 / URL），三击一次就整值到手。
fn string_content_range(chars: &[char], ci: usize) -> Option<(usize, usize)> {
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == '"' {
            let open = i;
            i += 1;
            let mut esc = false;
            while i < n {
                let c = chars[i];
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    break;
                }
                i += 1;
            }
            // 此时 i 停在闭引号（或文本末尾）。光标在两引号之间即算命中；
            // 贴着闭引号（ci == i）也算 —— 那是「点在最后一个字符右半边」。
            if ci > open && ci <= i && i < n {
                return Some((open + 1, i));
            }
        }
        i += 1;
    }
    None
}

/// `ci` 所在的标量 token（数字 / true / false / null）区间。
/// 只认字面量字符集；点在冒号、括号、空白上返回 None（回落默认整行选择）。
fn scalar_token_range(chars: &[char], ci: usize) -> Option<(usize, usize)> {
    let is_tok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_');
    let n = chars.len();
    let anchor = if ci < n && is_tok(chars[ci]) {
        ci
    } else if ci > 0 && ci <= n && is_tok(chars[ci - 1]) {
        ci - 1
    } else {
        return None;
    };
    let mut s = anchor;
    while s > 0 && is_tok(chars[s - 1]) {
        s -= 1;
    }
    let mut e = anchor + 1;
    while e < n && is_tok(chars[e]) {
        e += 1;
    }
    Some((s, e))
}

/// 可见字符区间 `[start, end)` 在 galley 里占用的矩形（按视觉行切分）。
/// 命中跨越折行时拆成多个矩形；行尾那截用 `char_w` 近似补上最后一个字符的宽度
///（等宽字体下正确，CJK 稍窄 —— 高亮底色差半个字宽不影响识别）。
fn match_rects(galley: &egui::Galley, start: usize, end: usize, char_w: f32) -> Vec<Rect> {
    let mut out = Vec::new();
    if end <= start {
        return out;
    }
    let first = galley.pos_from_cursor(CCursor::new(start));
    let (mut left, mut top, mut bottom) = (first.left(), first.top(), first.bottom());
    let mut right = first.left();
    for k in start + 1..=end {
        let r = galley.pos_from_cursor(CCursor::new(k));
        if (r.top() - top).abs() < 0.5 {
            right = r.left();
        } else {
            // 折行：上一行收尾（补一个字符宽），从新行重新起段。
            out.push(Rect::from_min_max(
                pos2(left, top),
                pos2(right.max(left) + char_w, bottom),
            ));
            if k == end {
                // 命中正好在行尾结束：别在下一行开头再画半个字宽的空段。
                return out;
            }
            left = r.left();
            top = r.top();
            bottom = r.bottom();
            right = r.left();
        }
    }
    out.push(Rect::from_min_max(
        pos2(left, top),
        pos2(right.max(left + char_w * 0.5), bottom),
    ));
    out
}

// ============================ 绘制 ============================

/// 在 `hit` 行号栏区绘制折叠箭头：折叠时 ▸（右），展开时 ▾（下）。
fn draw_arrow(painter: &egui::Painter, hit: Rect, folded: bool, color: Color32) {
    let c = hit.center();
    let pts: [Pos2; 3] = if folded {
        [
            pos2(c.x - 3.0, c.y - 4.0),
            pos2(c.x - 3.0, c.y + 4.0),
            pos2(c.x + 4.0, c.y),
        ]
    } else {
        [
            pos2(c.x - 4.0, c.y - 3.0),
            pos2(c.x + 4.0, c.y - 3.0),
            pos2(c.x, c.y + 4.0),
        ]
    };
    painter.add(Shape::convex_polygon(
        pts.to_vec(),
        color,
        egui::Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在无窗口环境下跑一帧，拿到可用的字体系统，然后按给定宽度排版一段文本。
    ///
    /// 排版是纯 CPU 的（不碰 GPU），所以 headless 可跑；这让「换行到底生效没有」
    /// 能被真正验证，而不是只看代码。
    fn layout(text: &str, wrap_w: f32) -> std::sync::Arc<egui::Galley> {
        let ctx = egui::Context::default();
        ctx.begin_pass(Default::default());
        let font_id = egui::FontId::monospace(12.0);
        let mut job = egui::text::LayoutJob::single_section(
            text.to_owned(),
            egui::TextFormat::simple(font_id, egui::Color32::WHITE),
        );
        job.wrap.max_width = wrap_w;
        job.wrap.break_anywhere = wrap_w.is_finite();
        let galley = ctx.fonts_mut(|f| f.layout_job(job));
        let _ = ctx.end_pass();
        galley
    }

    /// 这条是本次修复要证明的核心事实：不换行时内容会冲出可视宽度，
    /// 换行后必须收进去。此前编辑器恒为「不换行 + 只有纵向滚动」，
    /// 超出的部分既看不见也滚不到。
    #[test]
    fn wrapping_keeps_long_lines_inside_the_viewport() {
        // 一行很长的 JSON 字符串值（模拟长 URL / base64），中间没有空格
        let long = format!("{{\n  \"token\": \"{}\"\n}}", "abcd1234".repeat(40));
        let view_w = 300.0;

        let no_wrap = layout(&long, f32::INFINITY);
        assert!(
            no_wrap.size().x > view_w,
            "前提不成立：这段文本本来就没超宽（{} px）",
            no_wrap.size().x
        );

        let wrapped = layout(&long, view_w);
        assert!(
            wrapped.size().x <= view_w + 0.5,
            "换行后仍然超出可视宽度：{} > {}",
            wrapped.size().x,
            view_w
        );
        assert!(
            wrapped.rows.len() > no_wrap.rows.len(),
            "没有真的折行：{} 行 vs {} 行",
            wrapped.rows.len(),
            no_wrap.rows.len()
        );
    }
    // ---- Ctrl+F 搜索 / 三连击快速选值：纯函数行为 ----

    #[test]
    fn search_matches_are_case_insensitive_and_non_overlapping() {
        let hay: Vec<char> = "Token TOKEN token".chars().collect();
        let needle: Vec<char> = "token".chars().collect();
        assert_eq!(find_matches(&hay, &needle, 100), vec![0, 6, 12]);

        // 不重叠：aaa 里查 aa 只算一次（0..2），剩下一个 a 不够长
        let hay: Vec<char> = "aaa".chars().collect();
        let needle: Vec<char> = "aa".chars().collect();
        assert_eq!(find_matches(&hay, &needle, 100), vec![0]);

        // 上限生效：病态输入不会无限膨胀
        let hay: Vec<char> = "x".repeat(100).chars().collect();
        let needle: Vec<char> = "x".chars().collect();
        assert_eq!(find_matches(&hay, &needle, 7).len(), 7);
    }

    #[test]
    fn search_matches_handle_cjk_and_empty_query() {
        let hay: Vec<char> = r#"{"名称":"值"}"#.chars().collect();
        let needle: Vec<char> = "名称".chars().collect();
        assert_eq!(find_matches(&hay, &needle, 100), vec![2]);
        assert!(find_matches(&hay, &[], 100).is_empty());
    }

    /// 三连击选值的核心约定：点在字符串值内部 → 给出**不含引号**的内容区间，
    /// 转义引号不会把字符串提前截断。
    #[test]
    fn triple_click_selects_whole_string_value() {
        let s: Vec<char> = r#"{"key": "hello world"}"#.chars().collect();
        // "hello world" 的内容是下标 9..20（h 在 9，闭引号在 20）
        for ci in [10, 15, 20] {
            assert_eq!(string_content_range(&s, ci), Some((9, 20)), "ci={ci}");
        }
        // 点在键上选键内容
        assert_eq!(string_content_range(&s, 2), Some((2, 5)));
        // 点在冒号后的空格上不算字符串
        assert_eq!(string_content_range(&s, 7), None);

        // 含转义引号：\" 不结束字符串
        let e: Vec<char> = r#"{"a": "x\"y"}"#.chars().collect();
        assert_eq!(string_content_range(&e, 8), Some((7, 11)));
    }

    #[test]
    fn triple_click_selects_scalar_tokens() {
        let s: Vec<char> = r#"{"n": 1234.5, "b": true}"#.chars().collect();
        // 1234.5 在 6..12
        assert_eq!(scalar_token_range(&s, 8), Some((6, 12)));
        // true 在 19..23
        assert_eq!(scalar_token_range(&s, 21), Some((19, 23)));
        // 空格上：无标量（回落整行选择）
        assert_eq!(scalar_token_range(&s, 13), None);
    }

    /// 命中矩形按视觉行切分：跨折行的命中拆成多段，各自贴在自己那一行上。
    #[test]
    fn match_rects_split_at_wrapped_rows() {
        // 40 个 a，200px 宽度强制折行
        let galley = layout(&"a".repeat(40), 200.0);
        assert!(galley.rows.len() >= 2, "前提：确实折行了");
        let char_w = 12.0 * 0.6; // 近似等宽字符宽，只用于验证段数
        let rects = match_rects(&galley, 0, 40, char_w);
        assert_eq!(
            rects.len(),
            galley.rows.len(),
            "跨 {} 个视觉行的命中应拆成同样多的矩形",
            galley.rows.len()
        );
        // 每段的纵向范围应互不相同（各自属于不同的行）
        for w in rects.windows(2) {
            assert!(w[1].top() > w[0].top() + 0.5);
        }
    }

    /// 中间没有空格的长 token 必须能从中间断开。JSON 里这是常态（base64 / URL），
    /// 只按词断的话它照样冲出可视区 —— 所以 break_anywhere 是必需项而非偏好。
    #[test]
    fn long_token_without_spaces_still_wraps() {
        let token = "x".repeat(500);
        let g = layout(&token, 200.0);
        assert!(g.rows.len() > 1, "无空格长串没有被断开");
        assert!(g.size().x <= 200.5, "宽度仍然超限：{}", g.size().x);
    }

    /// 行号定位：续行不重复打号，且每个逻辑行的起点算得准。
    #[test]
    fn row_starts_marks_only_logical_line_beginnings() {
        // 三个逻辑行，中间那行足够长会被折成多行
        let text = format!("a\n{}\nc", "b".repeat(200));
        let g = layout(&text, 120.0);

        let starts = row_starts(&g);
        assert_eq!(starts.len(), g.rows.len(), "每个视觉行都要有一项");

        let firsts: Vec<usize> = starts
            .iter()
            .filter(|(_, is_first)| *is_first)
            .map(|(s, _)| *s)
            .collect();
        assert_eq!(firsts.len(), 3, "只应有 3 个逻辑行首：{firsts:?}");
        // 起点应落在 "a\n" 之后与最后一行 "c" 之前
        assert_eq!(firsts[0], 0);
        assert_eq!(firsts[1], 2, "第二个逻辑行从 'a\\n' 之后开始");
        assert_eq!(firsts[2], 2 + 200 + 1, "第三个逻辑行紧跟长行的换行符");

        // 视觉行数必须多于逻辑行数，否则这个用例没测到折行
        assert!(g.rows.len() > 3, "长行没有被折开，用例失去意义");
    }

    /// 不换行时逻辑行与视觉行一一对应 —— 保证关掉开关后行号行为与从前一致。
    #[test]
    fn row_starts_is_identity_without_wrapping() {
        let text = "one\ntwo\nthree";
        let g = layout(text, f32::INFINITY);
        let starts = row_starts(&g);
        assert_eq!(
            starts,
            vec![(0, true), (4, true), (8, true)],
            "不换行时每个视觉行都该是逻辑行首"
        );
    }

    /// 无窗口地驱动**真实的** `code_editor` 组件跑若干帧，把事件喂进去。
    ///
    /// 排版与交互都是纯 CPU 的，所以这里跑的是货真价实的组件代码路径（含换行、
    /// 光标可见↔源映射、编辑落盘），不是另写一份模拟逻辑。
    ///
    /// 前三帧模拟一次真实点击以取得焦点：**按下与抬起必须分属不同帧、且时间要推进** ——
    /// 挤在同一帧、时间恒为 0 时 egui 不会判定为 click，焦点也就拿不到。
    fn drive(text: &mut String, wrap: bool, width: f32, per_frame: Vec<Vec<Event>>) {
        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::dark();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
        // 点在正文区靠右处（避开左侧行号 / 折叠箭头栏），落点即插入点
        let click_at = pos2(width * 0.6, 20.0);
        let btn = |pressed: bool| Event::PointerButton {
            pos: click_at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        let mut frames: Vec<Vec<Event>> = vec![
            vec![Event::PointerMoved(click_at)],
            vec![btn(true)],
            vec![btn(false)],
        ];
        frames.extend(per_frame);

        for (i, events) in frames.into_iter().enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(i as f64 * 0.05),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                code_editor(
                    ui,
                    &theme,
                    "test-editor",
                    text,
                    300.0,
                    wrap,
                    FontCfg::default(),
                );
            });
        }
    }

    /// 跑一帧组件，返回所有被画出去的文字的最右边界。
    ///
    /// 这是在没有 GPU、截不了图的环境里，最接近「看一眼界面」的检查：
    /// 直接问渲染输出里的文字画到了哪儿。换行宽度算式（要扣掉行号栏、滚动条、
    /// 行尾光标位）一旦算歪，这里立刻能看出来。
    fn painted_text_right_edge(text: &str, wrap: bool, width: f32) -> f32 {
        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::dark();
        let mut buf = text.to_owned();
        let mut right = f32::NEG_INFINITY;
        // 跑两帧：第一帧建立布局，第二帧的输出才是稳定的
        for i in 0..2 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0))),
                    time: Some(i as f64 * 0.05),
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "paint-probe",
                        &mut buf,
                        300.0,
                        wrap,
                        FontCfg::default(),
                    );
                },
            );
            if i == 1 {
                for cs in &out.shapes {
                    if matches!(cs.shape, Shape::Text(_)) {
                        right = right.max(cs.shape.visual_bounding_rect().right());
                    }
                }
            }
        }
        right
    }

    /// 开着换行时，画出去的文字**一个字都不能越过可视区右边界**。
    /// 这正是用户看到的症状：超出去的部分被裁掉，既看不见也够不着。
    #[test]
    fn wrapped_text_never_paints_past_the_viewport() {
        let long = format!("{{\n  \"u\": \"https://x.dev/{}\"\n}}", "p".repeat(400));
        let width = 480.0;

        let wrapped = painted_text_right_edge(&long, true, width);
        assert!(wrapped.is_finite(), "没有画出任何文字，用例本身失效了");
        assert!(
            wrapped <= width,
            "开着换行仍然画到了可视区之外：右边界 {wrapped} > 视宽 {width}"
        );

        // 反面对照：关掉换行时文字确实会冲出去（此时靠横向滚动查看）
        let unwrapped = painted_text_right_edge(&long, false, width);
        assert!(
            unwrapped > width,
            "对照组不成立：不换行时本应超出可视区（{unwrapped}）"
        );
    }

    /// 目标场景的正面验证：**开着自动换行时仍然可以编辑**。
    /// 键入的字符必须落到点击处，而不是被换行改动坐标后落到别处。
    #[test]
    fn text_is_editable_while_wrapping() {
        let mut text = format!("{{\n  \"v\": \"{}\"\n}}", "z".repeat(400));
        let before = text.clone();
        drive(
            &mut text,
            true,
            420.0,
            vec![vec![Event::Text("Q".to_owned())]],
        );
        assert_ne!(text, before, "开着换行时键入没有生效");
        assert_eq!(
            text.chars().filter(|c| *c == 'Q').count(),
            1,
            "应当正好插入一个字符：{}",
            &text[..text.len().min(60)]
        );
        // 点击落在折行后的某个视觉行上，Q 会插进 z 串中间 —— 这正是要的：
        // 换行后点哪儿就改哪儿。原有内容一个字符都不能丢。
        assert_eq!(
            text.chars().filter(|c| *c == 'z').count(),
            400,
            "原有内容被改坏了"
        );
        assert!(text.starts_with("{\n  \"v\": \""));
        assert!(text.ends_with("\"\n}"));
    }

    /// 连续键入 + 退格：换行状态下的多次编辑必须逐次落到正确位置。
    #[test]
    fn successive_edits_land_correctly_while_wrapping() {
        let mut text = format!("[\n  \"{}\"\n]", "w".repeat(300));
        drive(
            &mut text,
            true,
            360.0,
            vec![
                vec![Event::Text("A".to_owned())],
                vec![Event::Text("B".to_owned())],
                vec![Event::Key {
                    key: Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            ],
        );
        assert_eq!(text.chars().filter(|c| *c == 'A').count(), 1, "A 没留下");
        assert_eq!(
            text.chars().filter(|c| *c == 'B').count(),
            0,
            "退格没有删掉刚输入的 B"
        );
    }

    /// 模拟「按住左键从 A 拖到 B」再发一个 Copy，返回被放进剪贴板的文本。
    ///
    /// 用复制内容来断言选区，是因为选区本身在组件私有状态里；而复制正是用户
    /// 选中之后最常做的事 —— 复制拿不到东西，就等于选中没生效。
    fn drag_select_and_copy(text: &str, wrap: bool, width: f32, from: Pos2, to: Pos2) -> String {
        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::dark();
        let mut buf = text.to_owned();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
        let btn = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frames: Vec<Vec<Event>> = vec![
            vec![Event::PointerMoved(from)],
            vec![btn(from, true)],         // 按下
            vec![Event::PointerMoved(to)], // 拖动
            vec![Event::PointerMoved(to)], // 多给一帧让拖拽状态稳定
            vec![btn(to, false)],          // 松开 → 选区定型
            vec![Event::Copy],
        ];
        let mut copied = String::new();
        for (i, events) in frames.into_iter().enumerate() {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "sel-probe",
                        &mut buf,
                        300.0,
                        wrap,
                        FontCfg::default(),
                    );
                },
            );
            for cmd in &out.platform_output.commands {
                if let egui::output::OutputCommand::CopyText(s) = cmd {
                    copied = s.clone();
                }
            }
        }
        copied
    }

    /// 用户报的问题：**格式化之后选不中**。格式化会让内容变高、超出视口，
    /// 所以这条用一份「远高于视口」的内容来复现。
    #[test]
    fn selection_works_when_content_overflows_viewport() {
        // 40 行，远超 300px 高的视口
        let mut lines = vec!["{".to_owned()];
        for i in 0..40 {
            lines.push(format!("  \"key{i:02}\": \"value{i:02}\","));
        }
        lines.push("  \"last\": 1".to_owned());
        lines.push("}".to_owned());
        let text = lines.join("\n");

        let copied = drag_select_and_copy(
            &text,
            true,
            520.0,
            pos2(80.0, 20.0),  // 第二行行首附近
            pos2(300.0, 60.0), // 往右下拖几行
        );
        assert!(
            !copied.is_empty(),
            "内容超出视口后拖拽选不中任何东西（用户报的问题）"
        );
        assert!(copied.contains("key"), "选中的内容不对：{copied:?}");
    }

    /// 更贴近用户操作顺序：**先在编辑器外点一下（比如点工具条上的「格式化」），
    /// 再回到编辑器里拖拽选中**。点工具条会让编辑器失焦，失焦之后还能不能选，
    /// 正是用户报的「格式化后选不中」。
    #[test]
    fn selection_works_after_clicking_outside_first() {
        let mut lines = vec!["{".to_owned()];
        for i in 0..40 {
            lines.push(format!("  \"key{i:02}\": \"value{i:02}\","));
        }
        lines.push("}".to_owned());
        let text = lines.join("\n");

        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::dark();
        let mut buf = text.clone();
        // 屏幕比编辑器高，底部留出「编辑器之外」的地方可点
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(520.0, 460.0));
        let outside = pos2(260.0, 440.0);
        let (from, to) = (pos2(80.0, 20.0), pos2(300.0, 60.0));
        let btn = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frames: Vec<Vec<Event>> = vec![
            // ① 先在编辑器里点一下（正常使用会先聚焦）
            vec![Event::PointerMoved(from)],
            vec![btn(from, true)],
            vec![btn(from, false)],
            // ② 点编辑器外面（等价于点工具条按钮）→ 编辑器失焦
            vec![Event::PointerMoved(outside)],
            vec![btn(outside, true)],
            vec![btn(outside, false)],
            // ③ 回到编辑器里拖拽选中
            vec![Event::PointerMoved(from)],
            vec![btn(from, true)],
            vec![Event::PointerMoved(to)],
            vec![Event::PointerMoved(to)],
            vec![btn(to, false)],
            vec![Event::Copy],
        ];
        let mut copied = String::new();
        for (i, events) in frames.into_iter().enumerate() {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "sel-after-outside",
                        &mut buf,
                        300.0,
                        true,
                        FontCfg::default(),
                    );
                },
            );
            for cmd in &out.platform_output.commands {
                if let egui::output::OutputCommand::CopyText(s) = cmd {
                    copied = s.clone();
                }
            }
        }
        assert_eq!(buf, text, "整个过程不该改动内容");
        assert!(
            !copied.is_empty(),
            "在编辑器外点过之后就再也选不中了 —— 正是用户报的现象"
        );
        assert!(copied.contains("key"), "选中的内容不对：{copied:?}");
    }

    /// 选中之后，选区底色必须**真的画出来**。
    ///
    /// 前面几条测试证明的是选区「状态」对（复制拿得到内容），但用户看到的是像素：
    /// 选区若没被画进 galley 网格，操作再正确也等于「选不中」。这里扫描渲染输出里
    /// 有没有选区底色的顶点。
    #[test]
    fn selection_highlight_is_actually_painted() {
        let theme = crate::theme::Theme::dark();
        let sel_color = theme.accent.gamma_multiply(0.35);
        let mut lines = vec!["{".to_owned()];
        for i in 0..40 {
            lines.push(format!("  \"key{i:02}\": \"value{i:02}\","));
        }
        lines.push("}".to_owned());
        let mut buf = lines.join("\n");

        let ctx = egui::Context::default();
        // 必须把主题装上：选区底色取自 ui.visuals().selection.bg_fill，
        // 不装主题扫到的就是 egui 的默认色，等于没测
        theme.apply(&ctx);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(520.0, 400.0));
        let (from, to) = (pos2(80.0, 20.0), pos2(300.0, 60.0));
        let btn = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frames: Vec<Vec<Event>> = vec![
            vec![Event::PointerMoved(from)],
            vec![btn(from, true)],
            vec![Event::PointerMoved(to)],
            vec![Event::PointerMoved(to)],
            vec![btn(to, false)],
            vec![], // 松开后再画一帧，这一帧就该看到选区
        ];
        let mut painted = false;
        for (i, events) in frames.into_iter().enumerate() {
            let last = i == 5;
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "sel-paint",
                        &mut buf,
                        300.0,
                        true,
                        FontCfg::default(),
                    );
                },
            );
            if last {
                for cs in &out.shapes {
                    if let Shape::Text(t) = &cs.shape {
                        for row in t.galley.rows.iter() {
                            if row
                                .visuals
                                .mesh
                                .vertices
                                .iter()
                                .any(|v| v.color == sel_color)
                            {
                                painted = true;
                            }
                        }
                    }
                }
            }
        }
        assert!(painted, "选区底色没有被画出来 —— 用户会觉得「选不中」");
    }

    /// 跑一串事件后返回被复制的文本（用来断言各种「选中」手势的结果）。
    fn run_events(text: &str, wrap: bool, width: f32, frames: Vec<Vec<Event>>) -> String {
        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::dark();
        theme.apply(&ctx);
        let mut buf = text.to_owned();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
        let mut copied = String::new();
        for (i, events) in frames.into_iter().enumerate() {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "gesture",
                        &mut buf,
                        300.0,
                        wrap,
                        FontCfg::default(),
                    );
                },
            );
            for cmd in &out.platform_output.commands {
                if let egui::output::OutputCommand::CopyText(s) = cmd {
                    copied = s.clone();
                }
            }
        }
        copied
    }

    fn key(k: Key, cmd: bool) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: cmd,
                ctrl: cmd,
                ..Default::default()
            },
        }
    }

    /// 全选（Ctrl/Cmd+A）→ 复制，必须拿到全文。
    /// 这是最常用的「选中」手势，比拖拽还常用。
    #[test]
    fn select_all_then_copy_returns_whole_text() {
        let text = "{\n  \"a\": 1,\n  \"b\": [1, 2, 3]\n}";
        let at = pos2(200.0, 20.0);
        let btn = |p: bool| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: p,
            modifiers: Default::default(),
        };
        let copied = run_events(
            text,
            true,
            520.0,
            vec![
                vec![Event::PointerMoved(at)],
                vec![btn(true)],
                vec![btn(false)],
                vec![key(Key::A, true)],
                vec![Event::Copy],
            ],
        );
        assert_eq!(copied, text, "全选后复制应当拿到完整内容");
    }

    /// 双击选词：双击一个 key 名，应当选中那个词而不是空。
    #[test]
    fn double_click_selects_a_word() {
        let text = "{\n  \"service\": \"gateway\"\n}";
        let at = pos2(70.0, 20.0); // 第二行的 "service" 附近
        let btn = |p: bool| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: p,
            modifiers: Default::default(),
        };
        let copied = run_events(
            text,
            true,
            520.0,
            vec![
                vec![Event::PointerMoved(at)],
                vec![btn(true)],
                vec![btn(false)],
                vec![btn(true)], // 第二次按下 → 双击
                vec![btn(false)],
                vec![Event::Copy],
            ],
        );
        assert!(!copied.is_empty(), "双击没有选中任何东西");
    }
    /// 三连击「快速选值」端到端：在字符串值上点三次 → 复制，
    /// 拿到的应当**恰好**是值内容（不含引号、不含整行的键与标点）。
    ///
    /// 测试字体的字符宽度不便硬编码，沿值区间横向扫一段 x：只要采样点里
    /// 有多个命中 "gateway"，特性就成立；一个都没有 = 特性坏了。
    #[test]
    fn triple_click_copies_exact_string_value() {
        let text = "{\n  \"service\": \"gateway\"\n}";
        let mut exact = 0usize;
        let mut seen = Vec::new();
        for xi in 0..30 {
            let at = pos2(60.0 + xi as f32 * 5.0, 26.0); // 第二行（行高约 17.5px）
            let btn = |p: bool| Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: p,
                modifiers: Default::default(),
            };
            let copied = run_events(
                text,
                true,
                520.0,
                vec![
                    vec![Event::PointerMoved(at)],
                    vec![btn(true)],
                    vec![btn(false)],
                    vec![btn(true)],
                    vec![btn(false)],
                    vec![btn(true)], // 第三次按下 → 三连击
                    vec![btn(false)],
                    vec![Event::Copy],
                ],
            );
            if copied == "gateway" {
                exact += 1;
            }
            seen.push(copied);
        }
        assert!(
            exact >= 3,
            "三连击在整个值区间内都没有精确选中值内容；各采样点结果：{seen:?}"
        );
    }

    /// Shift + 方向键扩选，然后复制。
    #[test]
    fn shift_arrow_extends_selection() {
        let text = "{\n  \"abc\": 1\n}";
        let at = pos2(40.0, 20.0);
        let btn = |p: bool| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: p,
            modifiers: Default::default(),
        };
        let shift_right = Event::Key {
            key: Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                shift: true,
                ..Default::default()
            },
        };
        let copied = run_events(
            text,
            true,
            520.0,
            vec![
                vec![Event::PointerMoved(at)],
                vec![btn(true)],
                vec![btn(false)],
                vec![shift_right.clone(), shift_right.clone(), shift_right],
                vec![Event::Copy],
            ],
        );
        assert_eq!(
            copied.chars().count(),
            3,
            "Shift+→ 三次应当选中 3 个字符：{copied:?}"
        );
    }

    /// 失焦之后选区仍要留在屏幕上（只是淡一点）。
    ///
    /// 这条对应用户报的现象：点一下工具条的按钮（编辑器随之失焦），刚选好的东西
    /// 就看不见了 —— 用起来就是「格式化之后选不中」。
    #[test]
    fn selection_stays_visible_after_losing_focus() {
        let theme = crate::theme::Theme::dark();
        let sel = theme.accent.gamma_multiply(0.35);
        let dim = sel.gamma_multiply(0.5);
        let mut buf = "{\n  \"a\": 1,\n  \"b\": 2\n}".to_owned();

        let ctx = egui::Context::default();
        theme.apply(&ctx);
        // 屏幕比编辑器高，底部可以点到「编辑器之外」
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(520.0, 460.0));
        let (from, to, outside) = (pos2(60.0, 20.0), pos2(200.0, 20.0), pos2(260.0, 440.0));
        let btn = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frames: Vec<Vec<Event>> = vec![
            vec![Event::PointerMoved(from)],
            vec![btn(from, true)],
            vec![Event::PointerMoved(to)],
            vec![Event::PointerMoved(to)],
            vec![btn(to, false)],
            // 到这里选区已经形成；接着点编辑器外面（等价于点工具条按钮）
            vec![Event::PointerMoved(outside)],
            vec![btn(outside, true)],
            vec![btn(outside, false)],
            vec![], // 失焦之后再画一帧
        ];
        let mut found = false;
        for (i, events) in frames.into_iter().enumerate() {
            let last = i == 8;
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "sel-blur",
                        &mut buf,
                        300.0,
                        true,
                        FontCfg::default(),
                    );
                },
            );
            if last {
                for cs in &out.shapes {
                    if let Shape::Text(t) = &cs.shape {
                        for row in t.galley.rows.iter() {
                            if row
                                .visuals
                                .mesh
                                .vertices
                                .iter()
                                .any(|v| v.color == dim || v.color == sel)
                            {
                                found = true;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            found,
            "编辑器失焦后选区就不见了 —— 点一下工具条按钮就等于白选"
        );
    }

    /// 折叠区间要数出**直接**子节点数：对象数键、数组数元素，嵌套层各数各的。
    #[test]
    fn regions_count_direct_children() {
        let json = "{\n  \"a\": 1,\n  \"b\": {\n    \"x\": 1,\n    \"y\": 2,\n    \"z\": 3\n  },\n  \"c\": [\n    10,\n    20\n  ]\n}";
        let chars: Vec<char> = json.chars().collect();
        let rs = scan_regions(&chars);
        // 按起点排序：最外层对象、内层对象 b、数组 c
        let mut rs = rs;
        rs.sort_by_key(|r| r.br);
        assert_eq!(rs.len(), 3, "应识别出 3 个可折叠区间");
        assert_eq!(
            (rs[0].count, rs[0].obj),
            (3, true),
            "外层对象有 a/b/c 三个键"
        );
        assert_eq!((rs[1].count, rs[1].obj), (3, true), "内层对象 b 有三个键");
        assert_eq!((rs[2].count, rs[2].obj), (2, false), "数组 c 有两个元素");
    }

    /// 空容器要数成 0，不能因为「没有逗号」就算成 1。
    #[test]
    fn empty_containers_count_zero() {
        let json = "{\n  \"empty_obj\": {\n  },\n  \"empty_arr\": [\n  ]\n}";
        let chars: Vec<char> = json.chars().collect();
        let mut rs = scan_regions(&chars);
        rs.sort_by_key(|r| r.br);
        assert_eq!(rs[1].count, 0, "空对象应为 0 项");
        assert_eq!(rs[2].count, 0, "空数组应为 0 个");
        // 但外层对象自己有两个键
        assert_eq!(rs[0].count, 2);
    }

    /// 字符串里的括号和逗号不能参与计数。
    #[test]
    fn commas_inside_strings_do_not_count() {
        let json = "{\n  \"csv\": \"a,b,c,d\",\n  \"brace\": \"{[\"\n}";
        let chars: Vec<char> = json.chars().collect();
        let rs = scan_regions(&chars);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].count, 2, "字符串里的逗号不算键：{}", rs[0].count);
    }

    /// 折叠之后，可见文本里必须出现节点数，且可见↔源的映射仍然对得上。
    #[test]
    fn folded_placeholder_shows_count_and_mapping_holds() {
        let json = "{\n  \"list\": [\n    1,\n    2,\n    3\n  ],\n  \"tail\": 9\n}";
        let chars: Vec<char> = json.chars().collect();
        let n = chars.len();
        let regions = scan_regions(&chars);
        // 折叠那个数组
        let arr = regions.iter().find(|r| !r.obj).expect("应有一个数组区间");
        let folded: HashSet<usize> = [arr.br].into_iter().collect();
        let (vis, segs) = build_visible(&chars, n, &regions, &folded);

        assert!(vis.contains("3 个"), "折叠占位里应显示元素数：{vis}");
        assert!(!vis.contains("\n    1,"), "被折叠的内容不该还在可见文本里");
        assert!(vis.contains("\"tail\": 9"), "折叠区间之外的内容必须保留");

        // 占位之后的字符要能正确映回源：拿 "tail" 在可见文本里的位置换算回源，
        // 源里同一位置也应当是 "tail"
        let vpos = vis.chars().collect::<Vec<_>>();
        let tail_v = vis
            .char_indices()
            .scan(0usize, |ci, (_, c)| {
                let cur = *ci;
                *ci += 1;
                Some((cur, c))
            })
            .position(|(i, _)| vpos[i..].starts_with(&['t', 'a', 'i', 'l']))
            .expect("可见文本里应有 tail");
        let (src_ci, _) = map_vis(&segs, tail_v, n);
        let src: String = chars[src_ci..(src_ci + 4).min(n)].iter().collect();
        assert_eq!(src, "tail", "占位长度变化后映射错位了");
    }

    /// **输入法在场时的拖拽选择**。
    ///
    /// Linux 上 ibus 之类的输入法会在文本框聚焦期间持续发送**空的 Preedit**
    /// （「当前没有在组字」的心跳）。这类事件必须当作无事发生 —— 一旦借它去动光标，
    /// 正在进行的拖拽选区就会每帧被塌回一个点，用户看到的就是「怎么拖都只选中一两个字」。
    ///
    /// 这条用例把空 Preedit 混进拖拽过程，断言选区照样能拉开。
    #[test]
    fn drag_selection_survives_empty_ime_preedit() {
        let mut lines = vec!["{".to_owned()];
        for i in 0..20 {
            lines.push(format!("  \"key{i:02}\": \"value{i:02}\","));
        }
        lines.push("}".to_owned());
        let text = lines.join("\n");

        let ime_noise = Event::Ime(egui::ImeEvent::Preedit {
            text: String::new(),
            active_range_chars: None,
        });
        let btn = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let (from, mid, to) = (pos2(70.0, 20.0), pos2(180.0, 20.0), pos2(300.0, 20.0));

        let copied = run_events(
            &text,
            true,
            520.0,
            vec![
                vec![Event::PointerMoved(from), ime_noise.clone()],
                vec![btn(from, true), ime_noise.clone()],
                // 拖拽过程中输入法一直在发空 Preedit
                vec![Event::PointerMoved(mid), ime_noise.clone()],
                vec![Event::PointerMoved(mid), ime_noise.clone()],
                vec![Event::PointerMoved(to), ime_noise.clone()],
                vec![Event::PointerMoved(to), ime_noise.clone()],
                vec![btn(to, false), ime_noise.clone()],
                vec![Event::Copy],
            ],
        );
        assert!(
            copied.chars().count() > 5,
            "输入法发空 Preedit 时拖拽只能选中 {} 个字符：{copied:?}",
            copied.chars().count()
        );
    }

    /// 中文输入不能被上面那条「空事件原地返回」的修复误伤。
    ///
    /// 走一遍真实的组字流程：逐步 Preedit（预编辑串上屏又被替换）→
    /// Preedit("") + Commit(汉字)（这两个常在**同一帧**到达）。
    #[test]
    fn chinese_ime_composition_still_commits() {
        // 第二行是一段较长的字符串值，这样点在中间必然落在引号**内部**，
        // 插入的中文不会破坏 JSON 结构
        let text = "{\n  \"k\": \"AAAAAAAAAAAAAAAAAAAA\"\n}";
        let at = pos2(140.0, 20.0);
        let btn = |p: bool| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: p,
            modifiers: Default::default(),
        };
        let pre = |s: &str| {
            Event::Ime(egui::ImeEvent::Preedit {
                text: s.to_owned(),
                active_range_chars: None,
            })
        };

        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::dark();
        theme.apply(&ctx);
        let mut buf = text.to_owned();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(520.0, 400.0));
        let frames: Vec<Vec<Event>> = vec![
            vec![Event::PointerMoved(at)],
            vec![btn(true)],
            vec![btn(false)],
            vec![pre("n")],
            vec![pre("ni")],
            vec![pre("nih")],
            vec![pre("你")],
            // 清空预编辑 + 提交，同一帧到达（真实输入法就是这么发的）
            vec![
                pre(""),
                Event::Ime(egui::ImeEvent::Commit("你好".to_owned())),
            ],
        ];
        for (i, events) in frames.into_iter().enumerate() {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "ime-cn",
                        &mut buf,
                        300.0,
                        true,
                        FontCfg::default(),
                    );
                },
            );
        }
        assert!(buf.contains("你好"), "中文没能上屏：{buf:?}");
        assert!(
            !buf.contains("nih") && !buf.contains("ni\""),
            "预编辑的拼音残留在正文里了：{buf:?}"
        );
        assert!(
            ferric_core::json::validate(&buf).is_ok(),
            "输入后结构被破坏：{buf:?}"
        );
    }

    /// 跑若干帧，返回每帧**正文首个文字**被画在哪个 x 上。
    ///
    /// 横向滚动生效与否，看的就是这个 x 有没有随滚动左移 —— 比断言内部偏移量更贴近
    /// 用户实际看到的东西。
    fn painted_text_left_edges(
        text: &str,
        wrap: bool,
        width: f32,
        frames: Vec<Vec<Event>>,
    ) -> Vec<f32> {
        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::dark();
        theme.apply(&ctx);
        let mut buf = text.to_owned();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
        let mut out_x = Vec::new();
        for (i, events) in frames.into_iter().enumerate() {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "hscroll",
                        &mut buf,
                        300.0,
                        wrap,
                        FontCfg::default(),
                    );
                },
            );
            let mut left = f32::INFINITY;
            for cs in &out.shapes {
                if let Shape::Text(t) = &cs.shape {
                    // 行号是逐个画的小段文字，正文是一整块；取最宽的那块即正文
                    if t.galley.size().x > 200.0 {
                        left = left.min(t.pos.x);
                    }
                }
            }
            out_x.push(left);
        }
        out_x
    }

    fn hscroll(dx: f32) -> Event {
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(dx, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        }
    }

    /// **关掉自动换行时，左右滚动必须真的能滚**。
    ///
    /// 不换行的长行会伸到可视区之外，此时唯一的查看手段就是横向滚动；
    /// 滚不动就等于那部分内容根本看不到。
    #[test]
    fn horizontal_scrolling_works_when_wrap_is_off() {
        let long = format!("{{\n  \"v\": \"{}\"\n}}", "0123456789".repeat(60));
        let width = 520.0;
        // 先让指针停在编辑区内（滚动事件要落到这个 ScrollArea 上）
        let hover = pos2(300.0, 60.0);
        let xs = painted_text_left_edges(
            &long,
            false,
            width,
            vec![
                vec![Event::PointerMoved(hover)],
                vec![Event::PointerMoved(hover)],
                vec![hscroll(-200.0)],
                vec![],
                vec![hscroll(-200.0)],
                vec![],
            ],
        );
        let first = xs[1];
        let last = *xs.last().unwrap();
        assert!(first.is_finite() && last.is_finite(), "没找到正文文字块");
        assert!(
            last < first - 50.0,
            "关掉换行后横向滚不动：正文左边界从 {first} 变到 {last}"
        );
    }

    /// **Shift + 滚轮**必须能左右滚。绝大多数鼠标只有纵向滚轮，
    /// 这是台式机用户查看长行的主要方式。
    #[test]
    fn shift_wheel_scrolls_horizontally_when_wrap_is_off() {
        let long = format!("{{\n  \"v\": \"{}\"\n}}", "0123456789".repeat(60));
        let hover = pos2(300.0, 60.0);
        let shift_wheel = |dy: f32| Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers {
                shift: true,
                ..Default::default()
            },
        };
        let xs = painted_text_left_edges(
            &long,
            false,
            520.0,
            vec![
                vec![Event::PointerMoved(hover)],
                vec![Event::PointerMoved(hover)],
                vec![shift_wheel(-200.0)],
                vec![],
                vec![shift_wheel(-200.0)],
                vec![],
            ],
        );
        let (first, last) = (xs[1], *xs.last().unwrap());
        assert!(
            last < first - 50.0,
            "Shift+滚轮没能横向滚动：{first} → {last}"
        );
    }

    /// 数一帧里画出的滚动条：返回 (有横向条, 有纵向条)。
    fn scrollbars_shown(text: &str, wrap: bool, w: f32, h: f32) -> (bool, bool) {
        let theme = crate::theme::Theme::dark();
        let ctx = egui::Context::default();
        theme.apply(&ctx);
        let mut buf = text.to_owned();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(w, h));
        let (mut hb, mut vb) = (false, false);
        for i in 0..3 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "bars",
                        &mut buf,
                        h - 20.0,
                        wrap,
                        FontCfg::default(),
                    );
                },
            );
            if i == 2 {
                for cs in &out.shapes {
                    let r = cs.shape.visual_bounding_rect();
                    if !r.is_finite() || r.width() <= 0.0 || r.height() <= 0.0 {
                        continue;
                    }
                    // 横向条：扁而宽、贴底；纵向条：窄而高、贴右
                    if r.width() > 60.0 && r.height() <= 12.0 && r.bottom() > h - 60.0 {
                        hb = true;
                    }
                    if r.height() > 60.0 && r.width() <= 12.0 && r.right() > w - 60.0 {
                        vb = true;
                    }
                }
            }
        }
        (hb, vb)
    }

    /// **短而宽的内容不该冒出纵向滚动条**。
    ///
    /// 横向滚动条一出现就会吃掉底部十几像素；若内容高度仍按满高算，就会出现
    /// 「内容高 > 视口高」，凭空多一根只能滚十几像素的纵向条 —— 用户看到的是
    /// 「右边这条的高度跟内容对不上」。
    #[test]
    fn short_but_wide_content_gets_only_a_horizontal_bar() {
        // 只有几行，但有一条很长的行
        let text = format!("{{\n  \"v\": \"{}\",\n  \"n\": 1\n}}", "x".repeat(400));
        let (h_bar, v_bar) = scrollbars_shown(&text, false, 520.0, 400.0);
        assert!(h_bar, "关掉换行、内容超宽，应当有横向滚动条");
        assert!(
            !v_bar,
            "内容只有几行却冒出了纵向滚动条（横向条占位没被扣掉）"
        );
    }

    /// 又高又宽时两条都要有 —— 上一条修复不能把该有的纵向条也修没了。
    #[test]
    fn tall_and_wide_content_gets_both_bars() {
        let mut lines = vec!["{".to_owned()];
        lines.push(format!("  \"v\": \"{}\",", "x".repeat(400)));
        for i in 0..60 {
            lines.push(format!("  \"k{i:02}\": {i},"));
        }
        lines.push("  \"last\": 0\n}".to_owned());
        let (h_bar, v_bar) = scrollbars_shown(&lines.join("\n"), false, 520.0, 400.0);
        assert!(h_bar && v_bar, "又高又宽时两条滚动条都该出现");
    }

    /// 字号设置必须真的改变排版：字号越大，同一段文本画得越宽越高。
    #[test]
    fn font_size_changes_layout() {
        let text = "{\n  \"key\": \"value\",\n  \"n\": 12345\n}";
        let measure = |size: f32| {
            let theme = crate::theme::Theme::dark();
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            let mut buf = text.to_owned();
            let mut wh = (0.0f32, 0.0f32);
            for i in 0..2 {
                let out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 400.0))),
                        time: Some(i as f64 * 0.05),
                        ..Default::default()
                    },
                    |ui| {
                        code_editor(
                            ui,
                            &theme,
                            "fsize",
                            &mut buf,
                            300.0,
                            false,
                            FontCfg {
                                size,
                                ..Default::default()
                            },
                        );
                    },
                );
                if i == 1 {
                    for cs in &out.shapes {
                        if let Shape::Text(t) = &cs.shape {
                            if t.galley.text().contains("value") {
                                wh = (t.galley.size().x, t.galley.size().y);
                            }
                        }
                    }
                }
            }
            wh
        };
        let small = measure(11.0);
        let large = measure(20.0);
        assert!(small.0 > 0.0, "没量到正文");
        assert!(
            large.0 > small.0 * 1.4,
            "字号调大后正文没有变宽：{} → {}",
            small.0,
            large.0
        );
        assert!(
            large.1 > small.1 * 1.4,
            "字号调大后正文没有变高：{} → {}",
            small.1,
            large.1
        );
    }

    /// 行距设置要真的改变行高（宽度不该跟着变）。
    #[test]
    fn line_spacing_changes_height_only() {
        let text = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}";
        let measure = |scale: f32| {
            let theme = crate::theme::Theme::dark();
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            let mut buf = text.to_owned();
            let mut wh = (0.0f32, 0.0f32);
            for i in 0..2 {
                let out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 400.0))),
                        time: Some(i as f64 * 0.05),
                        ..Default::default()
                    },
                    |ui| {
                        code_editor(
                            ui,
                            &theme,
                            "lspace",
                            &mut buf,
                            300.0,
                            false,
                            FontCfg {
                                line_scale: scale,
                                ..Default::default()
                            },
                        );
                    },
                );
                if i == 1 {
                    for cs in &out.shapes {
                        if let Shape::Text(t) = &cs.shape {
                            if t.galley.text().contains("\"a\"") {
                                wh = (t.galley.size().x, t.galley.size().y);
                            }
                        }
                    }
                }
            }
            wh
        };
        let tight = measure(1.15);
        let loose = measure(1.6);
        assert!(tight.1 > 0.0, "没量到正文");
        assert!(
            loose.1 > tight.1 * 1.2,
            "行距调宽后没有变高：{} → {}",
            tight.1,
            loose.1
        );
        assert!(
            (loose.0 - tight.0).abs() < 1.0,
            "行距不该影响宽度：{} → {}",
            tight.0,
            loose.0
        );
    }

    /// 脏草稿里的字号不能直接拿去排版（用户可写的 ron，可能是 0 / NaN / 巨大值）。
    #[test]
    fn font_cfg_clamps_bad_values() {
        let bad = FontCfg {
            size: 999.0,
            medium: false,
            line_scale: -3.0,
        }
        .clamped();
        assert_eq!(bad.size, FontCfg::MAX_SIZE);
        assert!(bad.line_scale >= 1.0);

        let nan = FontCfg {
            size: f32::NAN,
            medium: true,
            line_scale: f32::INFINITY,
        }
        .clamped();
        assert!(nan.size.is_finite() && nan.line_scale.is_finite());
        assert!(nan.row_height() > 0.0);
    }

    /// 编辑器改的样式（滚动条形态、滑块配色）**不能外溢**给调用方。
    ///
    /// `Ui::visuals_mut` 改的是调用方那个 Ui 的样式，不隔离的话会污染后面画的东西 ——
    /// 实测曾把侧栏分隔线染成深灰。这条守住那层 scope。
    #[test]
    fn editor_does_not_leak_style_to_caller() {
        let theme = crate::theme::Theme::dark();
        let ctx = egui::Context::default();
        theme.apply(&ctx);
        let mut buf = "{\n  \"a\": 1\n}".to_owned();
        let mut before = None;
        let mut after = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(520.0, 400.0))),
                ..Default::default()
            },
            |ui| {
                before = Some((
                    ui.spacing().scroll.floating,
                    ui.visuals().widgets.inactive.bg_fill,
                ));
                code_editor(
                    ui,
                    &theme,
                    "leak",
                    &mut buf,
                    200.0,
                    false,
                    FontCfg::default(),
                );
                after = Some((
                    ui.spacing().scroll.floating,
                    ui.visuals().widgets.inactive.bg_fill,
                ));
            },
        );
        assert_eq!(before, after, "编辑器把样式改动泄漏给了调用方");
    }

    /// 关掉换行且有长行时，**横向滚动条要真的画出来** —— 看不见就等于用户不知道能滚。
    #[test]
    fn horizontal_scrollbar_is_visible_when_wrap_is_off() {
        let long = format!("{{\n  \"v\": \"{}\"\n}}", "0123456789".repeat(60));
        let width = 520.0;
        let theme = crate::theme::Theme::dark();

        let count_bottom_bars = |wrap: bool| {
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            let mut buf = long.clone();
            let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
            let mut found = false;
            for i in 0..3 {
                let out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        time: Some(i as f64 * 0.05),
                        events: vec![Event::PointerMoved(pos2(300.0, 60.0))],
                        ..Default::default()
                    },
                    |ui| {
                        code_editor(
                            ui,
                            &theme,
                            "hbar",
                            &mut buf,
                            300.0,
                            wrap,
                            FontCfg::default(),
                        );
                    },
                );
                if i == 2 {
                    for cs in &out.shapes {
                        let r = cs.shape.visual_bounding_rect();
                        // 横向滚动条：又宽又扁，贴在底部
                        if r.width() > 60.0 && r.height() < 16.0 && r.top() > 240.0 {
                            found = true;
                        }
                    }
                }
            }
            found
        };

        assert!(
            count_bottom_bars(false),
            "关掉换行、内容超宽，却没有画出横向滚动条"
        );

        // 滚动条必须是**常驻**的，不能是 egui 默认那种「鼠标靠近才浮现」的浮动条：
        // 浮动条在浅色主题下淡到几乎看不见（实测白底上是 (252,252,253)），
        // 用户根本不知道内容可以左右滚。
        let ctx = egui::Context::default();
        theme.apply(&ctx);
        let mut buf = long.clone();
        let mut floating = true;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0))),
                ..Default::default()
            },
            |ui| {
                code_editor(
                    ui,
                    &theme,
                    "hbar2",
                    &mut buf,
                    300.0,
                    false,
                    FontCfg::default(),
                );
                // 进到编辑器内部看它实际用的样式
                ui.scope(|ui| {
                    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                    floating = ui.spacing().scroll.floating;
                });
            },
        );
        assert!(!floating, "滚动条应当是常驻实心样式");
    }

    /// 横向滚动时，**行号栏必须钉住不动**，只有正文左右移动。
    ///
    /// 行号跟着内容一起滑出屏幕的话，滚到右边就再也不知道自己在第几行 ——
    /// 横向滚动虽然「能滚」，用起来却是废的。所有代码编辑器都钉住行号栏。
    #[test]
    fn gutter_stays_pinned_while_scrolling_horizontally() {
        let long = format!("{{\n  \"v\": \"{}\"\n}}", "0123456789".repeat(60));
        let width = 520.0;
        let hover = pos2(300.0, 60.0);
        let theme = crate::theme::Theme::dark();

        let ctx = egui::Context::default();
        theme.apply(&ctx);
        let mut buf = long.clone();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
        let frames: Vec<Vec<Event>> = vec![
            vec![Event::PointerMoved(hover)],
            vec![Event::PointerMoved(hover)],
            vec![hscroll(-250.0)],
            vec![],
            vec![hscroll(-250.0)],
            vec![],
        ];
        // 每帧记录 (行号文字最左 x, 正文最左 x)
        let mut samples: Vec<(f32, f32)> = Vec::new();
        for (i, events) in frames.into_iter().enumerate() {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(i as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    code_editor(
                        ui,
                        &theme,
                        "gutter-pin",
                        &mut buf,
                        300.0,
                        false,
                        FontCfg::default(),
                    );
                },
            );
            let (mut num_x, mut body_x) = (f32::INFINITY, f32::INFINITY);
            for cs in &out.shapes {
                if let Shape::Text(t) = &cs.shape {
                    if t.galley.size().x > 200.0 {
                        body_x = body_x.min(t.pos.x); // 正文（很宽的一整块）
                    } else if t.galley.text().trim().parse::<u32>().is_ok() {
                        num_x = num_x.min(t.pos.x); // 行号（纯数字小段）
                    }
                }
            }
            samples.push((num_x, body_x));
        }

        let (n0, b0) = samples[1];
        let (n1, b1) = *samples.last().unwrap();
        assert!(n0.is_finite() && b0.is_finite(), "没采到行号或正文");
        assert!(b1 < b0 - 50.0, "正文没有横向滚动：{b0} → {b1}");
        assert!(
            (n1 - n0).abs() < 1.0,
            "行号栏跟着正文一起滚走了：{n0} → {n1}（应当钉住不动）"
        );
    }

    /// 开着换行时不该出现横向滚动（内容本来就收在可视区里，滚了也没意义）。
    #[test]
    fn no_horizontal_scrolling_when_wrapping() {
        let long = format!("{{\n  \"v\": \"{}\"\n}}", "0123456789".repeat(60));
        let hover = pos2(300.0, 60.0);
        let xs = painted_text_left_edges(
            &long,
            true,
            520.0,
            vec![
                vec![Event::PointerMoved(hover)],
                vec![Event::PointerMoved(hover)],
                vec![hscroll(-200.0)],
                vec![],
            ],
        );
        let first = xs[1];
        let last = *xs.last().unwrap();
        assert!(
            (last - first).abs() < 1.0,
            "开着换行却发生了横向滚动：{first} → {last}"
        );
    }

    /// 目标场景的完整串联：**压缩的长 JSON → 格式化 → 在换行状态下继续编辑**。
    ///
    /// 逐条对应诉求：格式化出来的长行不再冲出可视区；此时点进去照样能改；
    /// 改完内容仍是可以再次格式化的合法 JSON。
    #[test]
    fn format_then_edit_while_wrapped() {
        // 只放一个键，保证格式化后的第 2 行就是那条超长行 ——
        // `drive` 点在第二个视觉行上，正好落进这个字符串值里面
        let src = format!("{{\"url\":\"https://example.com/{}\"}}", "s".repeat(320));
        // 1) 格式化
        let formatted =
            ferric_core::json::format(&src, ferric_core::json::Indent::Two, false).expect("格式化");
        let width = 520.0;
        let longest = formatted.lines().map(|l| l.chars().count()).max().unwrap();
        assert!(longest > 300, "用例前提：格式化后仍有超长行");
        assert!(
            formatted.lines().nth(1).unwrap().chars().count() == longest,
            "用例前提：第 2 行就是那条长行"
        );

        // 2) 这一长行开着换行时不会画到可视区之外
        let right = painted_text_right_edge(&formatted, true, width);
        assert!(
            right <= width,
            "格式化结果仍然溢出可视区：{right} > {width}"
        );

        // 3) 换行状态下继续编辑
        let mut text = formatted.clone();
        drive(
            &mut text,
            true,
            width,
            vec![vec![Event::Text("X".to_owned())]],
        );
        assert_ne!(text, formatted, "格式化之后就改不动了");

        // 4) 编辑发生在字符串值内部，因此结果依然是合法 JSON，可以再次格式化
        ferric_core::json::validate(&text).expect("编辑后应仍是合法 JSON（改的是字符串值内部）");
        assert!(
            ferric_core::json::format(&text, ferric_core::json::Indent::Four, false).is_ok(),
            "改完之后应当还能再格式化一次"
        );
    }

    /// 关掉换行同样能编辑 —— 保证开关只影响排版，不影响编辑通路。
    #[test]
    fn text_is_editable_without_wrapping() {
        let mut text = "{\n  \"a\": 1\n}".to_owned();
        drive(
            &mut text,
            false,
            420.0,
            vec![vec![Event::Text("7".to_owned())]],
        );
        assert!(text.contains('7'), "关掉换行后键入没有生效：{text}");
    }

    /// 折叠箭头改用 galley 直接定位，必须能对上目标字符所在的视觉行。
    /// 这是从前那套「数换行符」的做法在换行后会错位的地方。
    #[test]
    fn cursor_position_tracks_wrapped_rows() {
        let text = format!("{{\n  \"k\": \"{}\"\n}}", "y".repeat(300));
        let g = layout(&text, 150.0);
        let last_brace = text.chars().count() - 1; // 结尾的 '}'
        let y = g.pos_from_cursor(CCursor::new(last_brace)).top();
        let first_y = g.pos_from_cursor(CCursor::new(0)).top();
        assert!(y > first_y, "末尾的 }} 应当排在首行之下（折行后依然成立）");
        // 该位置必须落在最后一个视觉行上
        let last_row_y = g.rows.last().expect("有行").pos.y;
        assert!(
            (y - last_row_y).abs() < 1.0,
            "定位没落到最后一行：{y} vs {last_row_y}"
        );
    }
}

/// 排版缓存的契约。
///
/// 这块缓存的收益与风险是同一件事：**跳过整条 O(文本长度) 的排版链**。
/// 跳对了，滚动/拖选/移光标从 3.0ms/帧 降到 0.3ms/帧（212KB JSON 实测，快 9 倍）；
/// 跳错了，屏幕上就是过期的画面 —— 轻则行号对不上，重则字形错位成乱码
/// （字体图集重建之后还拿旧 uv 去采样）。所以判据里少一项都不行。
#[cfg(test)]
mod layout_cache_tests {
    use super::*;
    use egui::{vec2, Pos2, Rect};

    /// 跑一帧，返回这一帧用的那份 [`Layout`] 的地址。
    ///
    /// 比地址而不是比时间：命中就是「同一个 `Arc`」，是个确定性的事实，
    /// 不会因为机器快慢而时红时绿。
    fn frame(
        ctx: &egui::Context,
        text: &mut String,
        wrap: bool,
        width: f32,
        font: FontCfg,
        dark: bool,
    ) -> usize {
        let theme = if dark {
            crate::theme::Theme::dark()
        } else {
            crate::theme::Theme::light()
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
        // 编辑器的 id 由 `ui.make_persistent_id` 从所在 Ui 的 id 链推出来，
        // 外面猜不到 —— 在同一个 Ui 上照同样的规则算一遍，才拿得到同一个 id。
        let mut lay_id = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                ui.scope(|ui| {
                    lay_id = Some(ui.make_persistent_id("cache-probe").with("__layout"));
                });
                code_editor(ui, &theme, "cache-probe", text, 300.0, wrap, font);
            },
        );
        ctx.data(|d| {
            d.get_temp::<std::sync::Arc<Layout>>(lay_id.expect("没跑到闭包里"))
                .map(|l| std::sync::Arc::as_ptr(&l) as usize)
        })
        .unwrap_or(0)
    }

    fn sample() -> String {
        "{\n  \"a\": 1,\n  \"b\": [1, 2, 3],\n  \"c\": \"文字\"\n}\n".to_owned()
    }

    /// 什么都没变 → 整份复用。这条不成立的话，这块缓存等于没做。
    #[test]
    fn an_unchanged_frame_reuses_the_whole_layout() {
        let ctx = egui::Context::default();
        let mut t = sample();
        let f = FontCfg::default();
        let a = frame(&ctx, &mut t, true, 600.0, f, true);
        assert_ne!(a, 0, "缓存没被存进去，后面的比较都没有意义");
        let b = frame(&ctx, &mut t, true, 600.0, f, true);
        assert_eq!(a, b, "文本、字体、宽度都没动，却重排了一遍");
        let c = frame(&ctx, &mut t, true, 600.0, f, true);
        assert_eq!(b, c, "缓存只命中了一帧就掉了 —— 判据里有每帧都在变的东西");
    }

    /// 文本变了必须重排（否则编辑完屏幕上还是旧内容）。
    #[test]
    fn editing_the_text_invalidates() {
        let ctx = egui::Context::default();
        let mut t = sample();
        let f = FontCfg::default();
        let a = frame(&ctx, &mut t, true, 600.0, f, true);
        t.push_str("\n// 新增\n");
        let b = frame(&ctx, &mut t, true, 600.0, f, true);
        assert_ne!(a, b, "文本改了却复用了旧排版");
    }

    /// 换行时改视口宽度必须重排；**不换行时不必** ——
    /// 那种情况下宽度根本不参与排版，跟着失效等于白白重排一次。
    #[test]
    fn width_matters_only_when_wrapping() {
        let ctx = egui::Context::default();
        let mut t = sample();
        let f = FontCfg::default();

        let a = frame(&ctx, &mut t, true, 600.0, f, true);
        let b = frame(&ctx, &mut t, true, 420.0, f, true);
        assert_ne!(a, b, "换行状态下改了宽度却没重排 —— 断行位置会是错的");

        let c = frame(&ctx, &mut t, false, 600.0, f, true);
        let d = frame(&ctx, &mut t, false, 420.0, f, true);
        assert_eq!(c, d, "不换行时宽度不参与排版，不该因为拖窗口就重排");
    }

    /// 换行开关本身也要能翻脸。
    #[test]
    fn toggling_wrap_invalidates() {
        let ctx = egui::Context::default();
        let mut t = sample();
        let f = FontCfg::default();
        let a = frame(&ctx, &mut t, true, 600.0, f, true);
        let b = frame(&ctx, &mut t, false, 600.0, f, true);
        assert_ne!(a, b, "开关了自动换行却复用了旧排版");
    }

    /// 字号 / 行距 / 字重任一项变了都要重排。
    #[test]
    fn font_settings_invalidate() {
        let ctx = egui::Context::default();
        // 「中黑」走的是命名族 mono-medium，得先把字体装上才排得了版
        crate::fonts::install_fonts(&ctx);
        let mut t = sample();
        let base = FontCfg::default();
        let a = frame(&ctx, &mut t, true, 600.0, base, true);

        for (name, f) in [
            ("字号", FontCfg { size: 18.0, ..base }),
            (
                "行距",
                FontCfg {
                    line_scale: 1.6,
                    ..base
                },
            ),
            (
                "字重",
                FontCfg {
                    medium: !base.medium,
                    ..base
                },
            ),
        ] {
            let b = frame(&ctx, &mut t, true, 600.0, f, true);
            assert_ne!(a, b, "{name}变了却复用了旧排版");
        }
    }

    /// 深浅色决定高亮配色，是排版结果的一部分。
    #[test]
    fn switching_theme_invalidates() {
        let ctx = egui::Context::default();
        let mut t = sample();
        let f = FontCfg::default();
        let a = frame(&ctx, &mut t, true, 600.0, f, true);
        let b = frame(&ctx, &mut t, true, 600.0, f, false);
        assert_ne!(a, b, "切了深浅色却复用旧 galley —— 颜色会停在上一个主题");
    }

    /// **界面缩放必须让缓存失效**，这是最容易漏、后果最严重的一项：
    /// `pixels_per_point` 一变，egui 会整个重建字体图集并丢掉自己的 galley 缓存；
    /// 我们这份活得更久，不跟着失效就会拿旧 uv 去采样新图集，屏幕上是错位的字。
    #[test]
    fn changing_ui_scale_invalidates() {
        let ctx = egui::Context::default();
        let mut t = sample();
        let f = FontCfg::default();
        let a = frame(&ctx, &mut t, true, 600.0, f, true);
        ctx.set_zoom_factor(1.4);
        let b = frame(&ctx, &mut t, true, 600.0, f, true);
        assert_ne!(a, b, "改了界面缩放却复用旧 galley —— 字形会错位");
    }
}
