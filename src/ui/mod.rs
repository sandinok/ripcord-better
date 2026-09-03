//! UI shell for Basalt. Discord 2025 layout:
//!
//! | 72px guilds | 240px channels | chat | 240px members |
//!
//! Every column is a real `egui::Panel`, so all of them stretch the full
//! window height. Includes the dark theme, the Basalt logo painter, and
//! small shared helpers.

#![allow(dead_code)]

pub mod chat;
pub mod emoji;
pub mod guilds_bar;
pub mod login;
pub mod members;
pub mod scroll;
pub mod settings;
pub mod sidebar;
pub mod reaction_picker;
pub mod squircle;

use egui::{Align, Color32, Layout, Sense, Ui};

use crate::colors;
use crate::state::ConnectionStatus;

/// Spawn a scoped child [`egui::Ui`] whose `max_rect` is the given `rect`.
pub fn allocate_ui_at_rect<R>(
    ui: &mut Ui,
    rect: egui::Rect,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        add_contents,
    )
    .inner
}

/// Sets the global egui style to the Discord 2025 dark theme.
pub fn apply_dark_theme(ctx: &egui::Context, use_legacy_status: bool) {
    let mut style = egui::Style {
        visuals: egui::Visuals::dark(),
        ..Default::default()
    };
    style.visuals.panel_fill = colors::BG_GUILDS_BAR;
    style.visuals.window_fill = colors::BG_FLOATING;
    style.visuals.extreme_bg_color = colors::BG_CHAT;
    style.visuals.faint_bg_color = colors::BG_SIDEBAR;
    style.visuals.widgets.hovered.bg_fill = colors::BG_ACCENT;
    style.visuals.widgets.active.bg_fill = colors::BLURPLE;
    style.visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.noninteractive.bg_fill = colors::BG_SIDEBAR;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_TERTIARY);
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_SECONDARY);
    style.visuals.hyperlink_color = colors::TEXT_LINK;
    style.visuals.selection.bg_fill = colors::BLURPLE;
    style.visuals.selection.stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);
    style.visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(96),
    };
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);
    // Tighten spacing globally for a denser, Discord-like feel.
    style.spacing.item_spacing = egui::vec2(8.0, 4.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.window_margin = egui::Margin::same(8);

    let _ = use_legacy_status;
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Returns the status color for the given connection status.
pub fn connection_color(s: ConnectionStatus) -> Color32 {
    match s {
        ConnectionStatus::Disconnected => colors::STATUS_OFFLINE,
        ConnectionStatus::Connecting => colors::STATUS_IDLE,
        ConnectionStatus::Connected => colors::STATUS_ONLINE,
        ConnectionStatus::AuthFailed | ConnectionStatus::DisallowedIntent => colors::STATUS_DND,
        ConnectionStatus::Reconnecting => colors::STATUS_IDLE,
    }
}

/// Returns the human-readable label for a connection status.
pub fn connection_label(s: ConnectionStatus) -> &'static str {
    match s {
        ConnectionStatus::Disconnected => "Connection lost - reconnecting",
        ConnectionStatus::Connecting => "Connecting to Discord",
        ConnectionStatus::Connected => "Connected",
        ConnectionStatus::AuthFailed => "Login failed - check your token",
        ConnectionStatus::DisallowedIntent => "Gateway rejected our intents",
        ConnectionStatus::Reconnecting => "Reconnecting",
    }
}

/// A clickable Material icon button. Returns true when clicked.
pub fn icon_button(
    ui: &mut Ui,
    name: &str,
    size: f32,
    color: Color32,
    id_source: &str,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size + 8.0), Sense::click());
    let response = ui
        .interact(rect, ui.id().with(id_source), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let draw_color = if response.hovered() {
        colors::TEXT_PRIMARY
    } else {
        color
    };
    crate::icons::draw(ui.painter(), name, rect.center(), size, draw_color);
    response.clicked()
}

// ───────────────────────── The Basalt logo ─────────────────────────
//
// Columnar basalt: three hexagonal stone columns, the center one taller.
// Drawn as vector polygons so it stays crisp at every size, from the
// 26-px home button up to the 88-px login mark.

/// Basalt brand colors for the logo.
pub mod logo {
    use egui::Color32;

    /// Squircle background top (lighter stone).
    pub const BG_TOP: Color32 = Color32::from_rgb(0x41, 0x46, 0x51);
    /// Squircle background bottom (dark basalt).
    pub const BG_BOTTOM: Color32 = Color32::from_rgb(0x26, 0x28, 0x2E);
    /// Column stone (light, cool gray).
    pub const COLUMN: Color32 = Color32::from_rgb(0xB6, 0xBE, 0xCB);
    /// Column stone (shaded side).
    pub const COLUMN_DARK: Color32 = Color32::from_rgb(0x8A, 0x93, 0xA3);
}

/// Draw the Basalt logo centered at (`cx`, `cy`) with side `size`.
pub fn draw_basalt_logo(painter: &egui::Painter, cx: f32, cy: f32, size: f32) {
    let rect = egui::Rect::from_center_size(egui::pos2(cx, cy), egui::Vec2::splat(size));
    let radius = size * 0.24;

    // Background squircle with a subtle vertical two-tone.
    painter.rect_filled(rect, radius, logo::BG_BOTTOM);
    let top_half = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.max.x, rect.center().y + size * 0.02),
    );
    painter.rect_filled(top_half, radius, logo::BG_TOP);

    // Three hexagonal columns: center full height, sides shorter.
    let col_w = size * 0.185; // column "radius" (pointy-top hexagon)
    let gap = size * 0.03;
    let center_cx = cx;
    let side_dx = col_w * 2.0 + gap + col_w * 0.5;
    let full_h = size * 0.62;
    let short_h = size * 0.44;

    // Center column (bright).
    draw_hex_column(painter, egui::pos2(center_cx, cy), col_w, full_h, logo::COLUMN, logo::COLUMN_DARK);
    // Side columns (slightly darker).
    let side_shade = logo::COLUMN.gamma_multiply(0.82);
    let side_shade_dark = logo::COLUMN_DARK.gamma_multiply(0.85);
    draw_hex_column(
        painter,
        egui::pos2(center_cx - side_dx, cy + size * 0.05),
        col_w,
        short_h,
        side_shade,
        side_shade_dark,
    );
    draw_hex_column(
        painter,
        egui::pos2(center_cx + side_dx, cy + size * 0.05),
        col_w,
        short_h,
        side_shade,
        side_shade_dark,
    );
}

/// A pointy-top hexagonal column: a stretched hexagon polygon.
/// `r` is the half-width; `h` is the total height.
fn draw_hex_column(
    painter: &egui::Painter,
    center: egui::Pos2,
    r: f32,
    h: f32,
    fill: Color32,
    shade: Color32,
) {
    let half_h = h / 2.0;
    let tip = h * 0.16; // vertical extent of the pointy tips
    // Pointy-top hexagon, clockwise from the top tip.
    let p1 = egui::pos2(center.x, center.y - half_h);
    let p2 = egui::pos2(center.x + r, center.y - half_h + tip);
    let p3 = egui::pos2(center.x + r, center.y + half_h - tip);
    let p4 = egui::pos2(center.x, center.y + half_h);
    let p5 = egui::pos2(center.x - r, center.y + half_h - tip);
    let p6 = egui::pos2(center.x - r, center.y - half_h + tip);
    let pts = vec![p1, p2, p3, p4, p5, p6];
    painter.add(egui::Shape::convex_polygon(pts.clone(), fill, egui::Stroke::NONE));
    // Right-side shading (a slim polygon along the right edge).
    let shade_pts = vec![
        p1,
        p2,
        p3,
        p4,
        egui::pos2(center.x + r * 0.45, p4.y - tip * 0.5),
        egui::pos2(center.x + r * 0.45, p1.y + tip * 0.5),
    ];
    painter.add(egui::Shape::convex_polygon(shade_pts, shade, egui::Stroke::NONE));
    let _ = shade;
}
