//! 自绘窗口外壳：标题栏、拖拽移动、最小化 / 最大化 / 关闭。

use crate::fonts::UI_SEMIBOLD;
use crate::icons;
use crate::theme::Theme;
use egui::{
    vec2, Align, Align2, Color32, FontFamily, FontId, Id, Layout, PointerButton, Rect, Sense, Ui,
    ViewportCommand,
};

/// 标题栏高度。
///
/// 从 46 收到 36：这条栏里只有左端一个应用名和右端三个窗口按钮，中间整段是死区，
/// 高度越大死区越显眼。36 仍然容得下 24px 的按钮块并留出上下呼吸，
/// 同时把内容区往上让了 10px。
pub const TITLE_BAR_HEIGHT: f32 = 36.0;

/// 窗口按钮的视觉块尺寸。**刻意比标题栏矮**：整条撑满的矩形（原先 44×46）
/// 悬停时是一大块生硬的方色带，而缩成带圆角的小块之后，悬停反馈看着像个按钮。
const WIN_BTN: (f32, f32) = (28.0, 24.0);

const CLOSE_HOVER: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);

/// 无边框窗口的边 / 角缩放：指针靠近窗口边缘时设缩放光标，主键按下即交给系统缩放。
/// 需在 `update` 顶层调用（面板绘制之前）。
pub fn handle_resize(ctx: &egui::Context) {
    use egui::viewport::ResizeDirection as D;
    use egui::CursorIcon as C;

    // 最大化时不缩放。
    if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
        return;
    }
    let Some(pos) = ctx.pointer_hover_pos() else {
        return;
    };
    let rect = ctx.input(|i| i.viewport_rect());
    let b = 6.0;
    let left = pos.x <= rect.left() + b;
    let right = pos.x >= rect.right() - b;
    let top = pos.y <= rect.top() + b;
    let bottom = pos.y >= rect.bottom() - b;

    let hit = if top && left {
        Some((D::NorthWest, C::ResizeNorthWest))
    } else if top && right {
        Some((D::NorthEast, C::ResizeNorthEast))
    } else if bottom && left {
        Some((D::SouthWest, C::ResizeSouthWest))
    } else if bottom && right {
        Some((D::SouthEast, C::ResizeSouthEast))
    } else if left {
        Some((D::West, C::ResizeWest))
    } else if right {
        Some((D::East, C::ResizeEast))
    } else if top {
        Some((D::North, C::ResizeNorth))
    } else if bottom {
        Some((D::South, C::ResizeSouth))
    } else {
        None
    };

    if let Some((dir, cursor)) = hit {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

fn toggle_maximize(ctx: &egui::Context) {
    let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}

fn win_btn(ui: &mut Ui, theme: &Theme, glyph: char, danger: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(WIN_BTN.0, WIN_BTN.1), Sense::click());
    let hovered = resp.hovered();
    if hovered {
        let fill = if danger { CLOSE_HOVER } else { theme.border };
        // 圆角与侧栏导航项、图标按钮同一族（7~9），别在这条栏里另起一套直角
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(7), fill);
    }
    let color = if danger && hovered {
        Color32::WHITE
    } else if hovered {
        theme.fg
    } else {
        theme.muted
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::new(12.0, icons::family()),
        color,
    );
    resp
}

/// 在标题栏面板内绘制内容并处理窗口交互。
pub fn title_bar_content(ui: &mut Ui, theme: &Theme) {
    let ctx = ui.ctx().clone();
    let rect: Rect = ui.max_rect();

    // 背景拖拽区（先注册，按钮后画覆盖在其上）
    let drag = ui.interact(rect, Id::new("titlebar-drag"), Sense::click_and_drag());
    // 指针底下压着浮层（设置窗、菜单等）时不接管拖拽：设置窗可以被拖到标题栏上方，
    // 在它上面按下应当是拖那个对话框，而不是把整个应用窗口拽着走。
    let over_overlay = ctx
        .pointer_interact_pos()
        .and_then(|p| ctx.layer_id_at(p))
        .is_some_and(|l| l.order != egui::Order::Background);
    if drag.drag_started_by(PointerButton::Primary) && !over_overlay {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if drag.double_clicked() && !over_overlay {
        toggle_maximize(&ctx);
    }

    // 应用名（Plus Jakarta Sans SemiBold）。字号压到 12 且用更淡的 muted：
    // 这里是窗口标识，不是内容标题 —— 它越安静，中间那段空白越不像「缺了点什么」。
    ui.painter().text(
        rect.left_center() + vec2(14.0, 0.0),
        Align2::LEFT_CENTER,
        "Ferric",
        FontId::new(12.0, FontFamily::Name(UI_SEMIBOLD.into())),
        theme.muted,
    );

    // 右侧窗口控制按钮：成组、贴右边留 6px，彼此 2px —— 三块等距铺开时
    // 看着像三个孤立的方块，收拢成一组才像窗口控件。
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.add_space(8.0);
            if win_btn(ui, theme, icons::X, true).clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            if win_btn(ui, theme, icons::SQUARE, false).clicked() {
                toggle_maximize(&ctx);
            }
            if win_btn(ui, theme, icons::MINUS, false).clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
            }
        });
    });

    // 底部细分隔线
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0_f32, theme.border),
    );
}
