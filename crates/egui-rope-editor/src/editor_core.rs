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

use crate::config::{Colors, FontConfig};
use crate::highlight::Highlighter;
use egui::text::CCursor;
use egui::text_selection::text_cursor_state::{cursor_rect, TextCursorState};
use egui::text_selection::visuals::{paint_cursor_end, paint_text_selection};
use egui::text_selection::CCursorRange;
use egui::{
    pos2, vec2, Align2, Color32, Event, EventFilter, Key, Modifiers, Pos2, Rect, Response, Sense,
    Shape, Ui, Vec2,
};
use std::collections::HashSet;

/// 一次编辑的最小信息（源 char 坐标）：`[lo, hi)` 被替换成 `added` 个字符。
///
/// 虚拟化路径据此**增量更新** char 数组 / 折叠区间 / 换行索引，避免每敲一个字符
/// 都全量重建（5MB 下全量 ~56ms，增量 ~11ms）。`has_struct` 表示这次编辑可能改变
/// 括号配对或换行结构（删了选区、或插入/删除了 `{}[]"\n`），此时折叠区间与换行
/// 索引退回全量重扫（其余结构仍是平移）。
#[derive(Clone, Copy)]
struct EditInfo {
    lo: usize,
    hi: usize,
    added: usize,
    has_struct: bool,
}

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
    /// 最近一次编辑（源 char 坐标），供大文件虚拟化路径做增量更新。
    edit: Option<EditInfo>,
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
        text: &ropey::Rope,
        folded: &HashSet<usize>,
        wrap_w: f32,
        font: &FontConfig,
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
        font.key().hash(&mut h);
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

/// 大文件虚拟化阈值（源字符数）。超过这个规模走「只排版可见区域」的路径：
/// egui 的 galley 每个字符约占 ~220 字节（glyph + 渲染网格），全文排版 5MB JSON
/// 就是 ~1.1GB。虚拟化后只对视口附近的行排版，内存降到几十 MB。
///
/// 注意：虚拟化路径强制不自动换行（行高恒定才好做按行裁剪），大文件里长行
/// 靠横向滚动查看 —— 与「关掉自动换行」的既有行为一致。
const VIRTUALIZE_CHARS: usize = 500_000;

/// 渲染一个可编辑、语法高亮、可折叠的代码编辑器，铺满 `height`，返回交互 `Response`。
///
/// `wrap = true` 时长行自动换行（不出现横向滚动条）；`false` 时保持长行不断，
/// 由横向滚动条查看超出部分。
#[allow(clippy::too_many_arguments)]
pub fn code_editor(
    ui: &mut Ui,
    theme: &Colors,
    id_source: &str,
    text: &mut ropey::Rope,
    height: f32,
    wrap: bool,
    font: FontConfig,
    highlighter: &dyn Highlighter,
) -> Response {
    // 整体套一层 scope：下面要改滚动条样式与滑块配色，而 `Ui::visuals_mut` 改的是
    // 调用方那个 Ui 的样式 —— 不隔离的话会外溢到别的界面元素上（实测把侧栏的
    // 分隔线染成了深灰）。scope 里的样式改动出了这个函数就作废。
    ui.scope(|ui| {
        if text.len_chars() > VIRTUALIZE_CHARS {
            code_editor_virtualized(ui, theme, id_source, text, height, wrap, font, highlighter)
        } else {
            code_editor_inner(ui, theme, id_source, text, height, wrap, font, highlighter)
        }
    })
    .inner
}

#[allow(clippy::too_many_arguments)]
fn code_editor_inner(
    ui: &mut Ui,
    theme: &Colors,
    id_source: &str,
    text: &mut ropey::Rope,
    height: f32,
    wrap: bool,
    font: FontConfig,
    highlighter: &dyn Highlighter,
) -> Response {
    let id = ui.make_persistent_id(id_source);
    // 排版一律走 FontConfig，不再读全局 TextStyle —— 字号/行距只影响这块编辑区
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
                ui.label(egui::RichText::new("⌕").size(13.0).color(theme.muted));

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
                if ui
                    .button("↑")
                    .on_hover_text("上一个（Shift+Enter / Shift+F3）")
                    .clicked()
                {
                    search.step(false);
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }
                if ui
                    .button("↓")
                    .on_hover_text("下一个（Enter / F3）")
                    .clicked()
                {
                    search.step(true);
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }
                if ui.button("×").on_hover_text("关闭（Esc）").clicked() {
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
                &font,
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

                    let (vis_chars, segs) = build_visible(&chars, n, &regions, &state.folded);
                    let vis: String = vis_chars.iter().collect();

                    let mut job =
                        highlighter.highlight(&vis, &font_id, theme, Some(row_h));
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
                            purpose: egui::IMEPurpose::Normal,
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

// ============================ 大文件虚拟化渲染 ============================
//
// 目标：5MB 级别的 JSON 也能流畅编辑。egui 的 galley 全文排版内存随字符数线性放大
//（实测 ~220 字节/字符，5MB ≈ 1.1GB）。虚拟化路径只对视口附近的行做 galley 排版，
// 内存与文件大小脱钩。
//
// 与既有全文路径的差异：
// - 视觉行 = 换行符自然分段 + 超长行按列折行（soft wrap）。这样单行 5MB 的压缩 JSON
//   也不会再把整行塞进一个 galley；`wrap` 开关因此在大文件下失效（总是折行保底）。
// - 滚动自己管理（不再用 ScrollArea 的全文内容）：滚轮 + 自绘滚动条 + 光标跟随。
// - 交互骨架（折叠、选区、搜索、编辑）仍复用下方的 char 级辅助函数，与全文路径一致。

/// 虚拟化路径的持久状态：滚动偏移 + 复用的编辑器/搜索状态。
#[derive(Clone, Default)]
struct VirtualState {
    scroll: Vec2,
    ed: EditorState,
    search: SearchState,
}

/// 一个视觉行：换行符自然分段，超长行再按列折行切成多段。
#[derive(Clone, Copy)]
struct VRow {
    /// 可见 char 起点。
    start: usize,
    /// 可见 char 终点（不含换行符）。
    end: usize,
    /// 是否为可见文本里换行符之后的第一段（列折行产生的续行不显示行号）。
    first: bool,
}

/// 虚拟化路径的全文 char 级结构（不含 galley，轻量，跨帧缓存）。
struct VirtualLayout {
    key: LayoutKey,
    /// 源 char 数。
    n: usize,
    /// 源文本 char 数组（增量更新的锚：编辑时 splice，结构字符变化才重扫）。
    ///
    /// 与 `vis_chars` 共用 `Arc`：未折叠时两者指向同一份，省一份 20MB 级的拷贝。
    chars: std::sync::Arc<Vec<char>>,
    regions: Vec<Region>,
    segs: Vec<Seg>,
    /// 折叠后的可见文本（char 数组：搜索/选区/光标都按 char 下标走）。
    vis_chars: std::sync::Arc<Vec<char>>,
    /// 视觉行索引（`start` 单调递增）。
    rows: Vec<VRow>,
    /// 源文本里每个换行符的 char 下标（可见行 → 源行号）。
    src_nl: Vec<usize>,
}

/// 滚动条预留宽度（自绘）。
const VSCROLL_W: f32 = 12.0;

/// 可见 char 下标所在的视觉行号（0 起）。
///
/// `rows[i].start` 单调递增，用二分定位。`v == rows[i].end`（行尾光标）时：
/// 若是列折行，下一行 `start == end`，于是 v 归到下一行行首 —— 这正是「同一位置
/// 两种叫法」；若是换行符，下一行 `start == end+1`，v 归到上一行末尾。
fn row_of(rows: &[VRow], v: usize) -> usize {
    rows.partition_point(|r| r.start <= v).saturating_sub(1)
}

/// 由可见文本 + 列宽构建视觉行索引：换行符自然分段，超长行按 `wrap_cols` 列折行。
///
/// 单行 5MB 的压缩 JSON 在这里被切成 `ceil(5M / wrap_cols)` 个视觉行，后续只对
/// 视口附近的行做 galley 排版 —— 不再有「整行塞进一个 galley」的 OOM。
#[allow(clippy::needless_range_loop)] // `i` 是断行点位置（0..=vis_len 含末尾），不是纯索引
fn build_rows(vis_chars: &[char], wrap_cols: usize) -> Vec<VRow> {
    let vis_len = vis_chars.len();
    let est = (vis_len / wrap_cols.max(1)).saturating_add(vis_len / 8).max(1);
    let mut rows = Vec::with_capacity(est);
    let mut cur = 0usize;
    let mut first = true;
    for i in 0..=vis_len {
        let at_break = i == vis_len || vis_chars[i] == '\n' || i - cur >= wrap_cols;
        if at_break {
            rows.push(VRow {
                start: cur,
                end: i,
                first,
            });
            if i < vis_len && vis_chars[i] == '\n' {
                cur = i + 1;
                first = true;
            } else {
                cur = i;
                first = false;
            }
        }
    }
    if rows.is_empty() {
        rows.push(VRow {
            start: 0,
            end: 0,
            first: true,
        });
    }
    rows
}

/// 增量更新虚拟化布局：把 `old`（上一帧的 char 级结构）按一次编辑 `e` 更新到当前 `text`。
///
/// 只 splice char 数组、平移折叠区间与换行索引，再重建可见文本与视觉行（常数小）。
/// 只有结构字符（`{}[]\"\n`）或删除选区时才全量重扫折叠区间 —— 于是最常见的
/// 「改一个值 / 打一个字母」从 O(n) 全量重建降到 ~O(编辑窗口 + 受影响区间)。
fn apply_edit(
    old: &VirtualLayout,
    e: EditInfo,
    text: &ropey::Rope,
    wrap_cols: usize,
    key: LayoutKey,
    folded: &HashSet<usize>,
) -> VirtualLayout {
    // 1. char 数组：make_mut 后 splice。未折叠时 chars 与 vis_chars 共享同一份
    //    Arc，make_mut 会自动 copy-on-write（只写这一份，旧份由 vis_chars 继续持有）。
    let mut chars = old.chars.clone();
    let ins: String = text
        .slice(text.char_to_byte(e.lo)..text.char_to_byte(e.lo + e.added))
        .to_string();
    {
        let chars_mut = std::sync::Arc::make_mut(&mut chars);
        chars_mut.reserve(e.added.saturating_sub(e.hi - e.lo));
        chars_mut.splice(e.lo..e.hi, ins.chars());
    }
    let n = chars.len();
    let delta = e.added as isize - (e.hi - e.lo) as isize;

    // 2. 折叠区间：平移；结构字符变化时重扫
    let mut regions = old.regions.clone();
    if e.has_struct {
        regions = scan_regions(&chars);
    } else {
        for r in &mut regions {
            if r.br >= e.lo {
                r.br = (r.br as isize + delta).max(0) as usize;
            }
            if r.open >= e.lo {
                r.open = (r.open as isize + delta).max(0) as usize;
            }
            if r.close >= e.lo {
                r.close = (r.close as isize + delta).max(0) as usize;
            }
        }
    }

    // 3. 换行索引：平移；结构字符变化时重扫
    let mut src_nl = old.src_nl.clone();
    if e.has_struct {
        src_nl = newline_positions(&chars);
    } else {
        for p in &mut src_nl {
            if *p >= e.lo {
                *p = (*p as isize + delta).max(0) as usize;
            }
        }
    }

    // 4. 可见文本：未折叠时与 chars 共享（省一份拷贝），折叠时重建；视觉行重建。
    let (vis_chars, segs) = if folded.is_empty() {
        (
            chars.clone(),
            vec![Seg {
                vis_start: 0,
                len: n,
                kind: SegKind::Real(0),
            }],
        )
    } else {
        let (v, s) = build_visible(&chars, n, &regions, folded);
        (std::sync::Arc::new(v), s)
    };
    let rows = build_rows(&vis_chars, wrap_cols);

    VirtualLayout {
        key,
        n,
        chars,
        regions,
        segs,
        vis_chars,
        rows,
        src_nl,
    }
}

/// 全量重建虚拟化布局（首次 / 结构性编辑 / 折叠变化时）。
fn build_virtual_layout(
    text: &ropey::Rope,
    wrap_cols: usize,
    key: LayoutKey,
    folded: &HashSet<usize>,
) -> VirtualLayout {
    let chars: std::sync::Arc<Vec<char>> = std::sync::Arc::new(text.chars().collect());
    let n = chars.len();
    let regions = scan_regions(&chars);
    let src_nl = newline_positions(&chars);
    let (vis_chars, segs) = if folded.is_empty() {
        (
            chars.clone(),
            vec![Seg {
                vis_start: 0,
                len: n,
                kind: SegKind::Real(0),
            }],
        )
    } else {
        let (v, s) = build_visible(&chars, n, &regions, folded);
        (std::sync::Arc::new(v), s)
    };
    let rows = build_rows(&vis_chars, wrap_cols);
    VirtualLayout {
        key,
        n,
        chars,
        regions,
        segs,
        vis_chars,
        rows,
        src_nl,
    }
}

#[allow(clippy::too_many_lines, clippy::needless_range_loop)] // 行号是全局行号，不是 enumerate 索引
#[allow(clippy::too_many_arguments)]
fn code_editor_virtualized(
    ui: &mut Ui,
    theme: &Colors,
    id_source: &str,
    text: &mut ropey::Rope,
    height: f32,
    _wrap: bool,
    font: FontConfig,
    highlighter: &dyn Highlighter,
) -> Response {
    let id = ui.make_persistent_id(id_source);
    let font = font.clamped();
    let font_id = font.font_id();
    let row_h = font.row_height();
    let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font_id, '0'));
    let num_color = ui.visuals().weak_text_color();
    let arrow_color = theme.muted;

    let mut st: VirtualState = ui
        .data_mut(|d| d.get_temp::<VirtualState>(id))
        .unwrap_or_default();

    // ---- Ctrl+F / F3 / Esc ----
    if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
        st.search.open = true;
        st.search.focus = true;
        st.search.prefill = true;
        st.ed.want_focus = false;
    }
    if st.search.open {
        if ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F3)) {
            st.search.step(false);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F3)) {
            st.search.step(true);
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            st.search.open = false;
            st.ed.want_focus = true;
        }
    }

    let inner_h = if st.search.open {
        (height - SEARCH_BAR_H).max(60.0)
    } else {
        height.max(60.0)
    };

    // ---- 搜索条（与全文路径同款）----
    if st.search.open {
        let bar = ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 32.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(2.0);
                ui.label(egui::RichText::new("⌕").size(13.0).color(theme.muted));
                let controls_w = 72.0 + 32.0 * 3.0 + 6.0 * 5.0;
                let input_w = (ui.available_width() - controls_w).max(120.0);
                let te = egui::TextEdit::singleline(&mut st.search.query)
                    .id(id.with("__vsearchbox"))
                    .desired_width(input_w)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("查找…（Enter 下一个 · Shift+Enter 上一个 · Esc 关闭）")
                    .show(ui);
                if st.search.focus {
                    st.search.focus = false;
                    te.response.request_focus();
                    st.search.select_all = true;
                }
                if st.search.select_all && te.response.has_focus() {
                    if let Some(mut ts) =
                        egui::text_edit::TextEditState::load(ui.ctx(), te.response.id)
                    {
                        let end = st.search.query.chars().count();
                        ts.cursor.set_char_range(Some(CCursorRange::two(
                            CCursor::new(0),
                            CCursor::new(end),
                        )));
                        ts.store(ui.ctx(), te.response.id);
                        st.search.select_all = false;
                    }
                }
                if te.response.changed() {
                    st.search.cur = 0;
                    st.search.goto = true;
                    ui.ctx().request_repaint();
                }
                if te.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    st.search.step(!ui.input(|i| i.modifiers.shift));
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }
                let (count, color) = if st.search.query.is_empty() {
                    (String::new(), theme.muted)
                } else if st.search.total == 0 {
                    ("无匹配".to_owned(), theme.danger)
                } else {
                    (format!("{}/{}", st.search.cur + 1, st.search.total), theme.muted)
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
                if ui.button("↑").on_hover_text("上一个").clicked() {
                    st.search.step(false);
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }
                if ui.button("↓").on_hover_text("下一个").clicked() {
                    st.search.step(true);
                    te.response.request_focus();
                    ui.ctx().request_repaint();
                }
                if ui.button("×").on_hover_text("关闭（Esc）").clicked() {
                    st.search.open = false;
                    st.ed.want_focus = true;
                }
            },
        );
        let r = bar.response.rect;
        ui.painter().hline(
            egui::Rangef::new(r.left(), r.right()),
            r.bottom() + 2.0,
            egui::Stroke::new(1.0_f32, theme.border),
        );
        ui.add_space(SEARCH_BAR_H - 32.0);
    }

    // ---- 布局缓存（仅 char 级结构，不含 galley）----
    //
    // 折行宽度 = 视口宽 - 行号栏预留。行号栏宽取决于源逻辑行数，而行号栏又要在
    // 排版前算好 —— 这里用 `text.len_lines()`（折叠只会减少、不会增加）做保守
    // 估算，避开与折行的循环依赖；真实 gutter 宽度在绘制时再精确算。
    let view_w = ui.available_width().max(120.0);
    let digits = text.len_lines().max(1).to_string().len().max(2);
    let gutter_est = char_w * digits as f32 + 26.0;
    let wrap_width = (view_w - VSCROLL_W - gutter_est - char_w).max(char_w * 8.0);
    let wrap_cols = (wrap_width / char_w).floor().max(1.0) as usize;

    let lay_id = id.with("__vlayout");
    let key = LayoutKey::new(
        text,
        &st.ed.folded,
        wrap_width,
        &font,
        theme.dark,
        ui.ctx().pixels_per_point(),
    );
    // 增量：本帧的编辑（若有）只作用于上一帧的 char 级结构，无需全量重建。
    let edit = st.ed.edit.take();
    let old = ui.data(|d| d.get_temp::<std::sync::Arc<VirtualLayout>>(lay_id));
    let layout = match (old, edit) {
        (Some(old), Some(e)) => {
            std::sync::Arc::new(apply_edit(&old, e, text, wrap_cols, key, &st.ed.folded))
        }
        (Some(old), None) if old.key == key => old,
        _ => std::sync::Arc::new(build_virtual_layout(
            text,
            wrap_cols,
            key,
            &st.ed.folded,
        )),
    };
    // 剪除失效的折叠锚点（编辑删掉某个开括号后，旧锚点不再对应任何区间）。
    let region_brs: HashSet<usize> = layout.regions.iter().map(|r| r.br).collect();
    st.ed.folded.retain(|br| region_brs.contains(br));
    ui.data_mut(|d| d.insert_temp(lay_id, layout.clone()));

    let n = layout.n;
    let segs = &layout.segs;
    let vis_chars = &layout.vis_chars;
    let vis_len = vis_chars.len();
    let rows = &layout.rows;
    let total_lines = rows.len();
    let gutter_w = gutter_width(layout.src_nl.len(), char_w);

    // ---- 视口 + 滚动范围 ----
    let vp_w = ui.available_width().max(120.0);
    let vp_h = inner_h;
    let (vp, _) = ui.allocate_exact_size(vec2(vp_w, vp_h), Sense::hover());
    let resp = ui.interact(vp, id, Sense::click_and_drag());

    let total_h = total_lines as f32 * row_h;
    let max_sy = (total_h - vp_h).max(0.0);

    // ---- 滚动输入（指针悬停在编辑器上时）----
    if resp.hovered() || resp.dragged() {
        let delta = ui.input(|i| i.smooth_scroll_delta);
        st.scroll.y = (st.scroll.y - delta.y).clamp(0.0, max_sy);
    }

    // ---- 可见行范围 ----
    let first = ((st.scroll.y / row_h).floor() as usize).min(total_lines.saturating_sub(1));
    let last = (((st.scroll.y + vp_h) / row_h).ceil() as usize)
        .min(total_lines.saturating_sub(1))
        .max(first);
    let fb = first.saturating_sub(8);
    let lb = (last + 8).min(total_lines.saturating_sub(1));

    // ---- 逐可见视觉行独立布局（只布局视口附近的行）----
    let mut galleys: Vec<std::sync::Arc<egui::Galley>> = Vec::with_capacity(lb - fb + 1);
    for li in fb..=lb {
        let line: String = vis_chars[rows[li].start..rows[li].end].iter().collect();
        let mut job = highlighter.highlight(&line, &font_id, theme, Some(row_h));
        job.wrap.max_width = f32::INFINITY;
        job.wrap.break_anywhere = false;
        galleys.push(ui.ctx().fonts_mut(|f| f.layout_job(job)));
    }

    let text_origin = pos2(
        vp.left() + gutter_w,
        vp.top() + fb as f32 * row_h - st.scroll.y,
    );

    // ---- 待映射光标（编辑/折叠后）----
    let mut follow_cursor = false;
    if let Some(src) = st.ed.pending.take() {
        let v = map_src(segs, src, vis_len);
        st.ed
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(v))));
        follow_cursor = true;
    }

    // ---- 折叠箭头命中框 ----
    let mut arrows: Vec<(Rect, usize, bool)> = Vec::new();
    for r in &layout.regions {
        if is_hidden(r.br, segs) {
            continue;
        }
        let v = map_src(segs, r.br, vis_len);
        let li = row_of(rows, v);
        if li < fb || li > lb {
            continue;
        }
        let y = text_origin.y + (li - fb) as f32 * row_h;
        arrows.push((
            Rect::from_min_size(pos2(vp.left() + 2.0, y), vec2(16.0, row_h)),
            r.br,
            st.ed.folded.contains(&r.br),
        ));
    }

    // ---- 折叠箭头点击 ----
    let mut gutter_click = false;
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            for (hit, br, _) in &arrows {
                if hit.contains(p) {
                    if let Some(rg) = st.ed.cursor.char_range() {
                        let (s, _) = map_vis(segs, rg.primary.index.0, n);
                        st.ed.pending = Some(s);
                    }
                    if st.ed.folded.contains(br) {
                        st.ed.folded.remove(br);
                    } else {
                        st.ed.folded.insert(*br);
                    }
                    gutter_click = true;
                    ui.ctx().request_repaint();
                    break;
                }
            }
        }
    }

    // ---- 焦点闩锁 ----
    let pressed_on_me =
        resp.clicked() || resp.dragged() || resp.is_pointer_button_down_on();
    let pressed_elsewhere = ui.input(|i| i.pointer.any_pressed()) && !resp.contains_pointer();
    if pressed_on_me && !gutter_click {
        st.ed.want_focus = true;
    }
    if pressed_elsewhere {
        st.ed.want_focus = false;
    }
    if st.ed.want_focus && !resp.has_focus() {
        resp.request_focus();
    }
    let has_focus = resp.has_focus();

    // ---- 鼠标定位 / 选择 ----
    //
    // 逐行布局后不能直接复用 `TextCursorState::pointer_interaction`（它会用单行 galley
    // clamp 整个选区，把跨行锚点压回当前行）。这里自己实现：单击定位、拖动 / Shift+点击
    // 扩展、双击选 token、三击选整行。
    if !gutter_click {
        if let Some(ptr) = resp.interact_pointer_pos() {
            let dragging = resp.dragged();
            if dragging || ptr.x >= vp.left() + gutter_w - 2.0 {
                let rel = ptr.y - text_origin.y;
                let off = (rel / row_h).floor().max(0.0) as usize;
                let li = (fb + off).min(lb);
                let g = &galleys[li - fb];
                let local = ptr - pos2(
                    text_origin.x,
                    text_origin.y + (li - fb) as f32 * row_h,
                );
                let ci = g.cursor_from_pos(local).index.0;
                let gv = rows[li].start + ci;
                if resp.double_clicked() {
                    let range = string_content_range(vis_chars, gv)
                        .or_else(|| scalar_token_range(vis_chars, gv))
                        .unwrap_or((rows[li].start, rows[li].end));
                    st.ed.cursor.set_char_range(Some(CCursorRange::two(
                        CCursor::new(range.0),
                        CCursor::new(range.1),
                    )));
                } else if resp.triple_clicked() {
                    st.ed.cursor.set_char_range(Some(CCursorRange::two(
                        CCursor::new(rows[li].start),
                        CCursor::new(rows[li].end),
                    )));
                } else if dragging {
                    if let Some(mut r) = st.ed.cursor.char_range() {
                        r.primary = CCursor::new(gv);
                        st.ed.cursor.set_char_range(Some(r));
                    }
                } else if resp.clicked() {
                    if ui.input(|i| i.modifiers.shift) {
                        if let Some(mut r) = st.ed.cursor.char_range() {
                            r.primary = CCursor::new(gv);
                            st.ed.cursor.set_char_range(Some(r));
                        }
                    } else {
                        st.ed
                            .cursor
                            .set_char_range(Some(CCursorRange::one(CCursor::new(gv))));
                    }
                }
            }
        }
    }

    // ---- 键盘 / 文本事件 ----
    if has_focus && !gutter_click {
        let events = ui.input(|i| i.events.clone());
        let mut vrange = st
            .ed
            .cursor
            .char_range()
            .unwrap_or_else(|| CCursorRange::one(CCursor::new(0)));
        'ev: for ev in &events {
            match ev {
                Event::Text(t) if !t.is_empty() => {
                    if edit_replace(text, segs, n, &mut st.ed, &vrange, t) {
                        ui.ctx().request_repaint();
                        break 'ev;
                    }
                }
                Event::Paste(t) if !t.is_empty() => {
                    if edit_replace(text, segs, n, &mut st.ed, &vrange, t) {
                        ui.ctx().request_repaint();
                        break 'ev;
                    }
                }
                Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    ..
                } => {
                    if edit_replace(text, segs, n, &mut st.ed, &vrange, "\n") {
                        ui.ctx().request_repaint();
                        break 'ev;
                    }
                }
                Event::Key {
                    key: Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    if edit_backspace(text, segs, n, &mut st.ed, &vrange) {
                        ui.ctx().request_repaint();
                        break 'ev;
                    }
                }
                Event::Key {
                    key: Key::Delete,
                    pressed: true,
                    ..
                } => {
                    if edit_delete(text, segs, n, vis_len, &mut st.ed, &vrange) {
                        ui.ctx().request_repaint();
                        break 'ev;
                    }
                }
                Event::Ime(ime) => {
                    edit_ime(text, segs, n, &mut st.ed, &vrange, ime);
                    ui.ctx().request_repaint();
                }
                Event::Copy => {
                    if let Some(s) = selection_src(text, segs, n, &vrange) {
                        if !s.is_empty() {
                            ui.ctx().copy_text(s);
                        }
                    }
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Ctrl/Cmd+A 全选、Ctrl+Home/End 跳首尾：与全文路径的
                    // `on_key_press` 行为对齐（虚拟化路径自己实现方向键，这几组也要补上）。
                    if modifiers.command {
                        match key {
                            Key::A => {
                                vrange = CCursorRange::two(
                                    CCursor::new(0),
                                    CCursor::new(vis_len),
                                );
                                continue 'ev;
                            }
                            Key::Home => {
                                vrange = CCursorRange::one(CCursor::new(0));
                                follow_cursor = true;
                                continue 'ev;
                            }
                            Key::End => {
                                vrange = CCursorRange::one(CCursor::new(vis_len));
                                follow_cursor = true;
                                continue 'ev;
                            }
                            _ => {}
                        }
                    }
                    let shift = modifiers.shift;
                    let v = vrange.primary.index.0;
                    let li = row_of(rows, v);
                    let ls = rows[li].start;
                    let le = rows[li].end;
                    let col = v - ls;
                    let new_v: Option<usize> = match key {
                        Key::ArrowUp if li > 0 => {
                            let tls = rows[li - 1].start;
                            let tle = rows[li - 1].end;
                            Some((tls + col).min(tle))
                        }
                        Key::ArrowDown if li + 1 < total_lines => {
                            let tls = rows[li + 1].start;
                            let tle = rows[li + 1].end;
                            Some((tls + col).min(tle))
                        }
                        Key::ArrowLeft => (v > 0).then_some(v - 1),
                        Key::ArrowRight => (v < vis_len).then_some(v + 1),
                        Key::Home => Some(ls),
                        Key::End => Some(le),
                        Key::PageUp | Key::PageDown => {
                            st.scroll.y = if *key == Key::PageUp {
                                (st.scroll.y - vp_h).max(0.0)
                            } else {
                                (st.scroll.y + vp_h).min(max_sy)
                            };
                            None
                        }
                        _ => None,
                    };
                    if let Some(nv) = new_v {
                        if shift {
                            vrange.primary = CCursor::new(nv);
                        } else {
                            vrange = CCursorRange::one(CCursor::new(nv));
                        }
                        follow_cursor = true;
                    }
                }
                _ => {}
            }
        }
        if st.ed.pending.is_none() {
            st.ed.cursor.set_char_range(Some(vrange));
        }
    }

    // ---- 搜索匹配 ----
    let mut match_list: Vec<usize> = Vec::new();
    let mut match_len = 0usize;
    if st.search.open {
        if st.search.prefill {
            st.search.prefill = false;
            if let Some(rg) = st.ed.cursor.char_range() {
                if !rg.is_empty() {
                    let [lo, hi] = rg.sorted_cursors();
                    st.search.query =
                        vis_chars[lo.index.0..hi.index.0].iter().collect::<String>();
                    st.search.select_all = true;
                    st.search.cur = 0;
                }
            }
        }
        let needle: Vec<char> = st.search.query.chars().take(256).collect();
        match_len = needle.len();
        if match_len > 0 && st.search.query.chars().count() <= 256 {
            match_list = find_matches(vis_chars, &needle, 200_000);
        }
        st.search.total = match_list.len();
        if st.search.cur >= st.search.total && st.search.total > 0 {
            st.search.cur = 0;
        }
    }

    // ---- 绘制 ----
    let painter = ui.painter().with_clip_rect(vp);

    // 选区（逐可见行着色）
    if let Some(r) = st.ed.cursor.char_range() {
        if !r.is_empty() {
            let [lo, hi] = r.sorted_cursors();
            let (sel_lo, sel_hi) = (lo.index.0, hi.index.0);
            let mut vis = ui.visuals().clone();
            if !has_focus {
                vis.selection.bg_fill = vis.selection.bg_fill.gamma_multiply(0.5);
            }
            for li in fb..=lb {
                let s = rows[li].start;
                let e = rows[li].end;
                let lo = sel_lo.max(s);
                let hi = sel_hi.min(e);
                if hi > lo {
                    paint_text_selection(
                        &mut galleys[li - fb],
                        &vis,
                        &CCursorRange::two(CCursor::new(lo - s), CCursor::new(hi - s)),
                        None,
                    );
                }
            }
        }
    }

    // 搜索命中底色（画在正文之下）
    if !match_list.is_empty() {
        let normal_bg = blend(theme.bg, theme.accent, 0.18);
        let cur_bg = blend(theme.bg, theme.accent, 0.42);
        for (mi, &s) in match_list.iter().enumerate() {
            let e = s + match_len;
            let lo_row = row_of(rows, s);
            let hi_row = row_of(rows, e.saturating_sub(1));
            for li in lo_row..=hi_row {
                if li < fb || li > lb {
                    continue;
                }
                let rs = rows[li].start;
                let re = rows[li].end;
                let lo = s.max(rs);
                let hi = e.min(re);
                if hi <= lo {
                    continue;
                }
                let is_cur = mi == st.search.cur;
                let g = &galleys[li - fb];
                for r in match_rects(g, lo - rs, hi - rs, char_w) {
                    let rr = r.translate(vec2(
                        text_origin.x,
                        text_origin.y + (li - fb) as f32 * row_h,
                    ));
                    if rr.bottom() < vp.top() || rr.top() > vp.bottom() {
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
    }

    // 正文（逐可见行）
    for li in fb..=lb {
        let y = text_origin.y + (li - fb) as f32 * row_h;
        painter.galley(pos2(text_origin.x, y), galleys[li - fb].clone(), theme.fg);
    }

    // 行号（折行续行不打号；折叠后号码跳变，与全文路径一致）
    for li in fb..=lb {
        if !rows[li].first {
            continue;
        }
        let (src_ci, _) = map_vis(segs, rows[li].start, n);
        let src_line = layout.src_nl.partition_point(|&p| p < src_ci) + 1;
        let y = text_origin.y + (li - fb) as f32 * row_h;
        painter.text(
            pos2(vp.left() + gutter_w - 8.0, y),
            Align2::RIGHT_TOP,
            src_line.to_string(),
            font_id.clone(),
            num_color,
        );
    }

    // 折叠箭头
    for (hit, _, folded) in &arrows {
        draw_arrow(&painter, *hit, *folded, arrow_color);
    }

    // 光标 + IME 候选框定位
    if has_focus {
        if let Some(r) = st.ed.cursor.char_range() {
            let p = r.primary.index.0;
            let li = row_of(rows, p);
            if li >= fb && li <= lb {
                let g = &galleys[li - fb];
                let pl = p - rows[li].start;
                let cr = cursor_rect(g, &CCursor::new(pl), row_h).translate(vec2(
                    text_origin.x,
                    text_origin.y + (li - fb) as f32 * row_h,
                ));
                paint_cursor_end(&painter, ui.visuals(), cr);
                ui.ctx().output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput {
                        purpose: egui::IMEPurpose::Normal,
                        rect: vp,
                        cursor_rect: cr,
                        should_interrupt_composition: false,
                    })
                });
            }
        }
    }

    // ---- 滚动条（自绘 + 拖拽，仅纵向）----
    if max_sy > 0.0 {
        let track = Rect::from_min_max(
            pos2(vp.right() - VSCROLL_W + 2.0, vp.top()),
            pos2(vp.right() - 2.0, vp.bottom()),
        );
        let thumb_h = (vp_h / total_h * vp_h).max(24.0);
        let thumb_y = vp.top() + (st.scroll.y / max_sy) * (vp_h - thumb_h);
        let thumb = Rect::from_min_max(
            pos2(track.left(), thumb_y),
            pos2(track.right(), thumb_y + thumb_h),
        );
        ui.painter()
            .rect_filled(track, 3.0, theme.faint.gamma_multiply(0.35));
        ui.painter().rect_filled(thumb, 3.0, theme.muted);
        if ui.interact(track, id.with("__vsb"), Sense::drag()).dragged() {
            if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                st.scroll.y =
                    ((p.y - vp.top() - thumb_h / 2.0) / (vp_h - thumb_h) * max_sy)
                        .clamp(0.0, max_sy);
            }
        }
    }

    // ---- 光标跟随滚动 ----
    if follow_cursor {
        if let Some(r) = st.ed.cursor.char_range() {
            let v = r.primary.index.0;
            let li = row_of(rows, v);
            let y = li as f32 * row_h;
            if y < st.scroll.y {
                st.scroll.y = y;
            } else if y + row_h > st.scroll.y + vp_h {
                st.scroll.y = (y + row_h - vp_h).max(0.0);
            }
            st.scroll.y = st.scroll.y.clamp(0.0, max_sy);
        }
    }

    // ---- 搜索跳转 ----
    if st.search.goto {
        st.search.goto = false;
        if let Some(&s) = match_list.get(st.search.cur) {
            let li = row_of(rows, s);
            st.scroll.y = (li as f32 * row_h - vp_h / 2.0).clamp(0.0, max_sy);
            ui.ctx().request_repaint();
        }
    }

    ui.data_mut(|d| d.insert_temp(id, st));
    resp
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
) -> (Vec<char>, Vec<Seg>) {
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

    // 直接构建 char 数组：可见↔源映射与搜索/选区都按 char 下标走，
    // 省掉「String → Vec<char>」的二次拷贝。需要 &str 时由调用方 `iter().collect()`。
    let mut vis: Vec<char> = Vec::with_capacity(n);
    let mut segs: Vec<Seg> = Vec::with_capacity(top.len() + 1);
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
            vis.extend_from_slice(&chars[si..r.open]);
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
        vis.extend(ph.chars());
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
        vis.extend_from_slice(&chars[si..n]);
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
// —— Rope 文本操作：按 char 下标删除/插入（ropey 的 insert/remove 本身就是 char 下标）——
/// 该字符是否可能改变 JSON 的括号配对或换行结构（决定增量更新能否只平移折叠区间）。
fn is_struct_char(c: char) -> bool {
    // `\\` 也必须算：字符串里输入反斜杠会改变「下一个引号是否结束字符串」的判定，
    // 进而改变后续括号是否被算作结构。
    matches!(c, '{' | '}' | '[' | ']' | '"' | '\\' | '\n' | '\r')
}

/// 合并同帧内的两次编辑（专用于 IME：同一预编辑位置连续「删旧串 + 插新串」）。
///
/// 每次编辑的 `lo` 都是同一个起点、`hi` 单调，最终 `added` 即最终文本长度，
/// 于是合并就是「取旧区间的最小起点 / 最大终点 + 最终长度」。
fn merge_edit(a: EditInfo, b: EditInfo) -> EditInfo {
    EditInfo {
        lo: a.lo.min(b.lo),
        hi: a.hi.max(b.hi),
        added: b.added,
        has_struct: a.has_struct || b.has_struct,
    }
}

fn rope_delete_char_range(rope: &mut ropey::Rope, lo: usize, hi: usize) {
    if lo < hi {
        rope.remove(lo..hi);
    }
}

fn rope_insert_text(rope: &mut ropey::Rope, ins: &str, at: usize) -> usize {
    rope.insert(at, ins);
    ins.chars().count()
}

fn edit_replace(
    text: &mut ropey::Rope,
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
        rope_delete_char_range(text, lo, hi);
    }
    let added = rope_insert_text(text, ins, lo);
    let delta = added as isize - (hi - lo) as isize;
    shift_folds(state, lo, delta);
    state.pending = Some(lo + added);
    state.edit = Some(EditInfo {
        lo,
        hi,
        added,
        has_struct: hi > lo || ins.chars().any(is_struct_char),
    });
    true
}

fn edit_backspace(
    text: &mut ropey::Rope,
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
    let c = text.char(p - 1);
    rope_delete_char_range(text, p - 1, p);
    shift_folds(state, p - 1, -1);
    state.pending = Some(p - 1);
    state.edit = Some(EditInfo {
        lo: p - 1,
        hi: p,
        added: 0,
        has_struct: is_struct_char(c),
    });
    true
}

fn edit_delete(
    text: &mut ropey::Rope,
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
    let c = text.char(p);
    rope_delete_char_range(text, p, p + 1);
    shift_folds(state, p, -1);
    state.pending = Some(p);
    state.edit = Some(EditInfo {
        lo: p,
        hi: p + 1,
        added: 0,
        has_struct: is_struct_char(c),
    });
    true
}

/// 处理输入法事件（组字 Preedit / 提交 Commit）。参照 egui `TextEdit`：预编辑串直接写入
/// 源文本并被下一帧渲染，每次新 Preedit 先删除上次预编辑区间再重插；Commit 落定为正式文本。
fn edit_ime(
    text: &mut ropey::Rope,
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
        rope_delete_char_range(text, s, e);
        shift_folds(state, s, -((e - s) as isize));
        // 删除旧预编辑串：合并进本帧的累积编辑（连续 Preedit 靠它把旧串抵消掉）
        let del = EditInfo {
            lo: s,
            hi: e,
            added: 0,
            has_struct: false,
        };
        state.edit = Some(state.edit.take().map_or(del, |p| merge_edit(p, del)));
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
                let added = rope_insert_text(text, t, start);
                shift_folds(state, start, added as isize);
                state.ime = Some((start, start + added));
                state.pending = Some(start + added);
                let ins = EditInfo {
                    lo: start,
                    hi: start,
                    added,
                    has_struct: t.chars().any(is_struct_char),
                };
                state.edit = Some(state.edit.take().map_or(ins, |p| merge_edit(p, ins)));
            }
            true
        }
        egui::ImeEvent::Commit(t) => {
            let added = rope_insert_text(text, t, start);
            shift_folds(state, start, added as isize);
            state.pending = Some(start + added);
            let ins = EditInfo {
                lo: start,
                hi: start,
                added,
                has_struct: t.chars().any(is_struct_char),
            };
            state.edit = Some(state.edit.take().map_or(ins, |p| merge_edit(p, ins)));
            true
        }
        _ => {
            state.pending = Some(start);
            false
        }
    }
}

/// 取当前选区对应的**源文本**切片（用于复制/剪切）；跨占位则返回含隐藏内容的整段。
fn selection_src(text: &ropey::Rope, segs: &[Seg], n: usize, vrange: &CCursorRange) -> Option<String> {
    let (p, _) = map_vis(segs, vrange.primary.index.0, n);
    let (s, _) = map_vis(segs, vrange.secondary.index.0, n);
    let (lo, hi) = (p.min(s), p.max(s));
    if hi <= lo {
        return Some(String::new());
    }
    let b_lo = text.char_to_byte(lo);
    let b_hi = text.char_to_byte(hi);
    Some(text.slice(b_lo..b_hi).to_string())
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
    use crate::{CodeEditor, Colors};

    fn drive(text: &mut ropey::Rope, wrap: bool, per_frame: Vec<Vec<egui::Event>>) {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0));
        let click_at = egui::pos2(400.0, 20.0);
        let btn = |pressed: bool| egui::Event::PointerButton {
            pos: click_at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut frames = vec![
            vec![egui::Event::PointerMoved(click_at)],
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
            let mut out = ctx.run_ui(input, |ui| {
                CodeEditor::new("test-editor")
                    .height(300.0)
                    .wrap(wrap)
                    .colors(Colors::dark())
                    .show(ui, text);
            });
            // 单测不接 GPU，字体纹理 delta 无法消费，drop 前 clear 掉。
            out.textures_delta.clear();
        }
    }

    /// 小文件走全文路径，编辑落点正确。
    #[test]
    fn edits_small_text() {
        let mut buf = ropey::Rope::from_str("{\n  \"a\": 1\n}");
        drive(
            &mut buf,
            true,
            vec![vec![egui::Event::Text("X".to_owned())]],
        );
        assert!(buf.to_string().contains('X'), "编辑应生效：{}", buf);
    }

    /// 大文件走虚拟化路径，单行超长文本不 OOM、编辑生效。
    #[test]
    fn edits_huge_single_line() {
        let s = "1234567890".repeat(60_000);
        let mut buf = ropey::Rope::from_str(&s);
        assert!(buf.len_chars() > VIRTUALIZE_CHARS);
        drive(
            &mut buf,
            false,
            vec![vec![egui::Event::Text("X".to_owned())]],
        );
        assert!(buf.to_string().contains('X'), "单行超长文本下编辑也应生效");
    }

    /// 折叠区间扫描：忽略字符串内的括号，数出直接子节点数。
    #[test]
    fn scans_regions_ignoring_strings() {
        let chars: Vec<char> = "{\n  \"a\": \"{}\",\n  \"b\": [\n    1,\n    2\n  ]\n}".chars().collect();
        let regions = scan_regions(&chars);
        assert_eq!(regions.len(), 2, "应有对象和数组两个区间");
        let obj = regions.iter().find(|r| r.obj).expect("对象区间");
        let arr = regions.iter().find(|r| !r.obj).expect("数组区间");
        assert_eq!(obj.count, 2, "对象数 2 个键");
        assert_eq!(arr.count, 2, "数组数 2 个元素");
    }
}

