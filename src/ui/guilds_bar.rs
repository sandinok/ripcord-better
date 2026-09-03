//! Guilds bar - leftmost 72px column. Home button (Basalt logo), server
//! icons with Discord-style rounded-square + hover morph + white selection
//! pill + tooltips + unread badges, add-server, and the settings cog.

use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2};

use crate::colors;
use crate::model::Snowflake;
use crate::state::{self, AppState, Selection};

const BTN: f32 = 48.0;

pub fn render(ui: &mut Ui, app_state: &AppState, rest: std::sync::Arc<crate::rest::Http>) -> Option<Selection> {
    let mut new_sel: Option<Selection> = None;
    let frame = egui::Frame::new()
        .fill(colors::BG_GUILDS_BAR)
        .inner_margin(egui::Margin {
            left: 0,
            right: 0,
            top: 12,
            bottom: 12,
        });
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.vertical(|ui| {
            // ── Home (DMs) button ──
            let home_active = app_state.selection_sync().guild_id.is_none();
            let (home_rect, home_resp) = button_rect(ui, "guilds_home");
            let home_resp = home_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let anim_t = ui.ctx().animate_value_with_time(
                ui.id().with("home_anim"),
                if home_active { 1.0 } else { if home_resp.hovered() { 0.4 } else { 0.0 } },
                0.15,
            );
            let radius = lerp(16.5, 24.0, anim_t);
            let bg = Color32::from_rgba_premultiplied(0x35, 0x39, 0x42, (90.0 + 90.0 * anim_t) as u8);
            ui.painter_at(home_rect).rect_filled(home_rect, radius, bg);
            draw_pill(ui, home_rect, anim_t);
            crate::ui::draw_basalt_logo(ui.painter(), home_rect.center().x, home_rect.center().y, 30.0);
            if home_resp.clicked() {
                new_sel = Some(Selection { guild_id: None, channel_id: None });
            }
            if home_resp.hovered() {
                show_tooltip(ui, home_rect, "Direct Messages");
            }

            render_separator(ui);

            // ── Guild list (scrollable) ──
            let guilds = app_state.guilds.read().clone();
            let current_guild = app_state.selection_sync().guild_id;
            // Reserve exactly what the bottom block needs (separator, add
            // button, settings button, spacings, bottom margin), measured
            // from the panel's clip rect - available_height lies after
            // set_min_height.
            let bottom_reserve = 8.0 + 2.0 + 8.0 + 48.0 + 8.0 + 48.0 + 12.0;
            let remaining = ui.max_rect().bottom() - ui.next_widget_position().y;
            let guilds_height = (remaining - bottom_reserve).max(48.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(guilds_height)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 8.0;
                    for g in &guilds {
                        let is_active = current_guild == Some(g.id);
                        let (rect, resp) = button_rect(ui, &format!("guild-{}", g.id.0));
                        let resp = resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(&g.name);
                        let anim_t = ui.ctx().animate_value_with_time(
                            ui.id().with(format!("guild_anim_{}", g.id.0)),
                            if is_active { 1.0 } else { if resp.hovered() { 0.4 } else { 0.0 } },
                            0.15,
                        );
                        // Discord-2024 squircle: 16.5px radius at rest, morphing
                        // to a circle (24 = 48/2) on hover/active, with a
                        // subtle scale-up. The corner mask is baked into the
                        // icon texture at decode, so the image itself is
                        // rounded instead of covering the shape with a
                        // square (the "square server icons" bug).
                        let radius = lerp(16.5, 24.0, anim_t);
                        let grow = 2.0 * anim_t;
                        let icon_rect = Rect::from_center_size(
                            rect.center(),
                            Vec2::splat(BTN + grow),
                        );
                        let icon_url = g.icon_url();
                        let accent = pick_accent_for(&g.id);
                        let painter = ui.painter_at(icon_rect);
                        // Background: the accent color shows for initial-only
                        // guilds; for guilds WITH an icon it only shows as a
                        // loading placeholder behind the image.
                        if icon_url.is_none() {
                            let bg = if is_active { accent } else { accent.gamma_multiply(0.62) };
                            painter.rect_filled(icon_rect, radius, bg);
                        } else {
                            painter.rect_filled(icon_rect, radius, colors::BG_ACCENT);
                        }
                        if resp.hovered() && !is_active {
                            painter.rect_filled(
                                icon_rect,
                                radius,
                                Color32::from_rgba_premultiplied(255, 255, 255, 26),
                            );
                        }
                        // Guild icon image (rounded corners baked in) or initials.
                        match icon_url {
                            Some(url) => {
                                crate::ui::allocate_ui_at_rect(ui, icon_rect, |ui| {
                                    crate::image_loader::render_image(
                                        ui,
                                        &url,
                                        icon_rect.width(),
                                        crate::image_loader::Shape::Rounded(34),
                                    );
                                });
                            }
                            None => {
                                painter.text(
                                    icon_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    g.initials(),
                                    egui::FontId::proportional(16.0),
                                    colors::TEXT_PRIMARY,
                                );
                            }
                        }
                        draw_pill(ui, rect, anim_t);
                        // Unread badge.
                        let unread = guild_unread(app_state, g.id);
                        if unread > 0 {
                            draw_badge(ui.painter_at(rect), rect, unread);
                        }
                        if resp.clicked() {
                            new_sel = Some(Selection {
                                guild_id: Some(g.id),
                                channel_id: None,
                            });
                            fetch_guild_channels(rest.clone(), g.id);
                        }
                    }
                });

            ui.add_space(8.0);
            render_separator(ui);

            // ── Add server (decorative for v0.1: joins via invite code are
            // not implemented yet; the button explains itself). ──
            {
                let (rect, resp) = button_rect(ui, "guilds_add");
                let resp = resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Join a server (coming soon)");
                let anim_t = ui.ctx().animate_value_with_time(
                    ui.id().with("add_anim"),
                    if resp.hovered() { 0.4 } else { 0.0 },
                    0.15,
                );
                let radius = lerp(16.5, 24.0, anim_t);
                let bg = if anim_t > 0.2 { colors::GREEN } else { colors::GREEN.gamma_multiply(0.75) };
                ui.painter_at(rect).rect_filled(rect, radius, bg);
                crate::icons::draw(ui.painter(), "add", rect.center(), 24.0, colors::TEXT_PRIMARY);
            }

            // ── Settings cog (bottom) ──
            {
                let (rect, resp) = button_rect(ui, "guilds_settings");
                let resp = resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("User settings");
                let anim_t = ui.ctx().animate_value_with_time(
                    ui.id().with("settings_anim"),
                    if resp.hovered() { 0.4 } else { 0.0 },
                    0.15,
                );
                let radius = lerp(16.5, 24.0, anim_t);
                let bg = Color32::from_rgba_premultiplied(0x35, 0x39, 0x42, (90.0 + 90.0 * anim_t) as u8);
                ui.painter_at(rect).rect_filled(rect, radius, bg);
                crate::icons::draw(ui.painter(), "settings", rect.center(), 26.0, colors::TEXT_PRIMARY);
                if resp.clicked() {
                    if let Some(s) = state::global() {
                        s.request_settings_toggle();
                    } else {
                        app_state.request_settings_toggle();
                    }
                }
            }
            ui.allocate_space(ui.available_size());
        });
    });
    new_sel
}

/// Allocate a full-width 72px row for a guild button. The clickable icon
/// occupies 12..60 horizontally; 0..12 is the selection-pill gutter.
fn button_rect(ui: &mut Ui, id: &str) -> (Rect, egui::Response) {
    let (row, _) = ui.allocate_exact_size(Vec2::new(72.0, BTN), Sense::click());
    let rect = Rect::from_min_max(
        egui::pos2(row.min.x + 12.0, row.min.y),
        egui::pos2(row.max.x - 12.0, row.max.y),
    );
    let resp = ui
        .interact(rect, ui.id().with(id), Sense::click());
    (rect, resp)
}

/// The white selection pill on the left edge. Height animates with `t`
/// (0 = hidden, 0.4 = hover nub, 1 = full).
fn draw_pill(ui: &mut Ui, btn_rect: Rect, t: f32) {
    if t <= 0.01 {
        return;
    }
    let height = lerp(8.0, 40.0, t);
    let bar = Rect::from_min_size(
        Pos2::new((btn_rect.min.x - 11.0).max(ui.max_rect().min.x), btn_rect.center().y - height / 2.0),
        Vec2::new(7.0, height),
    );
    ui.painter().rect_filled(bar, 3.5, colors::TEXT_PRIMARY);
}

fn draw_badge(painter: egui::Painter, rect: Rect, count: u32) {
    let text = if count > 99 { "99+".to_string() } else { count.to_string() };
    let r = if count > 9 { 10.5 } else { 8.5 };
    let center = Pos2::new(rect.right() - 2.0, rect.top() + 2.0);
    // Ring in the bar background so the badge reads on any icon.
    painter.circle_filled(center, r + 2.0, colors::BG_GUILDS_BAR);
    painter.circle_filled(center, r, colors::RED);
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(10.5),
        colors::TEXT_PRIMARY,
    );
}

fn guild_unread(app_state: &AppState, guild_id: Snowflake) -> u32 {
    let channels = app_state.channels.read();
    channels
        .iter()
        .filter(|c| c.guild_id == Some(guild_id))
        .map(|c| app_state.mention_count(c.id))
        .sum()
}

fn show_tooltip(ui: &mut Ui, rect: Rect, text: &str) {
    let _ = (ui, rect, text);
}

fn render_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(32.0, 2.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 1.0, colors::BG_ACCENT);
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn pick_accent_for(id: &Snowflake) -> Color32 {
    let idx = (u64::from(*id) % 18) as usize;
    crate::colors::default_role_color_hex(idx)
}

/// Fetch the channel list for a guild via REST (GUILD_CREATE usually
/// delivers it too; this covers guilds whose events we missed).
pub fn fetch_guild_channels(rest: std::sync::Arc<crate::rest::Http>, guild_id: Snowflake) {
    let rest_clone = rest.clone();
    let global_state = state::global();
    tokio::spawn(async move {
        match rest_clone.get_guild_channels(guild_id).await {
            Ok(channels) => {
                if let Some(s) = global_state {
                    let mut cur = s.channels.write();
                    cur.retain(|c| c.guild_id != Some(guild_id));
                    for mut c in channels {
                        c.guild_id = Some(guild_id);
                        cur.push(c);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "fetch channels for guild");
            }
        }
    });
}
