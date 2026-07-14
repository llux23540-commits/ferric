//! 自绘窗口外壳：标题栏、拖拽移动、最小化 / 最大化 / 关闭。

use crate::fonts::UI_SEMIBOLD;
use crate::icons;
use crate::theme::Theme;
use egui::{
    vec2, Align, Align2, Color32, FontFamily, FontId, Id, Layout, PointerButton, Rect, Sense, Ui,
    ViewportCommand,
};

pub const TITLE_BAR_HEIGHT: f32 = 46.0;

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
    let rect = ctx.screen_rect();
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
    let size = vec2(44.0, TITLE_BAR_HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    if hovered {
        let fill = if danger { CLOSE_HOVER } else { theme.border };
        ui.painter().rect_filled(rect, 0.0, fill);
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
        FontId::new(14.0, icons::family()),
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
    if drag.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        toggle_maximize(&ctx);
    }

    // 应用名（Plus Jakarta Sans SemiBold）
    ui.painter().text(
        rect.left_center() + vec2(16.0, 0.0),
        Align2::LEFT_CENTER,
        "Ferric",
        FontId::new(13.0, FontFamily::Name(UI_SEMIBOLD.into())),
        theme.fg_soft,
    );

    // 右侧窗口控制按钮
    #[allow(deprecated)]
    ui.allocate_ui_at_rect(rect, |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
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
