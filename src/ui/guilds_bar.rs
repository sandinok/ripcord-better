//! Guilds bar - leftmost 72px column. Home button (Basalt logo), server
//! icons with Discord-style rounded-square + hover morph + white selection
//! pill + tooltips + unread badges, add-server, and the settings cog.

use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2};

use crate::colors;
use crate::model::Snowflake;
use crate::state::{self, AppState, Selection};

const BTN: f32 = 48.0;

pub fn render(
    ui: &mut Ui,
    app_state: &AppState,
    rest: std::sync::Arc<crate::rest::Http>,
    config: &mut crate::config::Config,
) -> Option<Selection> {
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

                    // ── Folders first, then plain guilds (point 24) ──
                    let foldered_ids: std::collections::HashSet<u64> = config
                        .guild_folders
                        .iter()
                        .flat_map(|f| f.guild_ids.iter().filter_map(|id| id.parse::<u64>().ok()))
                        .collect();

                    let mut folder_ctx: Option<usize> = None;
                    let mut guild_ctx: Option<Snowflake> = None;
                    let mut open_folder: Option<usize> = None;

                    for (idx, folder) in config.guild_folders.iter().enumerate() {
                        let members: Vec<&crate::model::Guild> = folder
                            .guild_ids
                            .iter()
                            .filter_map(|id| id.parse::<u64>().ok())
                            .filter_map(|id| guilds.iter().find(|g| g.id.0 == id))
                            .collect();
                        let folder_unread: u32 = members.iter().map(|g| guild_unread(app_state, g.id)).sum();
                        let active = members
                            .iter()
                            .any(|g| current_guild == Some(g.id));
                        let expanded = ui
                            .ctx()
                            .data(|d| d.get_temp::<bool>(egui::Id::new("folder_open").with(idx)))
                            .unwrap_or(false);
                        let (rect, resp) = button_rect(ui, &format!("folder-{idx}"));
                        let resp = resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(&folder.name);
                        let anim_t = ui.ctx().animate_value_with_time(
                            ui.id().with(format!("folder_anim_{idx}")),
                            if active { 1.0 } else { if resp.hovered() { 0.4 } else { 0.0 } },
                            0.15,
                        );
                        let radius = lerp(16.5, 24.0, anim_t);
                        let painter = ui.painter_at(rect);
                        let bg = if active {
                            colors::BG_ACCENT
                        } else {
                            Color32::from_rgba_premultiplied(0x35, 0x39, 0x42, (90.0 + 90.0 * anim_t) as u8)
                        };
                        painter.rect_filled(rect, radius, bg);
                        // Folder face: 2x2 mini icons of the first 4 members.
                        draw_folder_face(ui, rect, &members);
                        draw_pill(ui, rect, anim_t);
                        if folder_unread > 0 {
                            draw_badge(ui.painter_at(rect), rect, folder_unread);
                        }
                        if resp.secondary_clicked() {
                            folder_ctx = Some(idx);
                        }
                        if resp.clicked() {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("folder_open").with(idx), !expanded)
                            });
                        }
                        if expanded {
                            open_folder = Some(idx);
                        }
                    }

                    for g in &guilds {
                        if foldered_ids.contains(&g.id.0) {
                            continue;
                        }
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
                        // Guild icon image (rounded corners baked in) or
                        // initials. Animated (a_ / .gif) icons PLAY on hover
                        // (point 11): the animated decoder swaps frames only
                        // while the pointer is over the icon.
                        match icon_url.as_deref() {
                            Some(url) if url.ends_with(".gif") => {
                                crate::ui::allocate_ui_at_rect(ui, icon_rect, |ui| {
                                    crate::image_loader::render_animated_image(
                                        ui,
                                        url,
                                        Vec2::splat(icon_rect.width()),
                                        crate::image_loader::Shape::Rounded(34),
                                        resp.hovered() || is_active,
                                    );
                                });
                            }
                            Some(url) => {
                                crate::ui::allocate_ui_at_rect(ui, icon_rect, |ui| {
                                    crate::image_loader::render_image(
                                        ui,
                                        url,
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
                        if resp.secondary_clicked() {
                            guild_ctx = Some(g.id);
                        }
                        if resp.clicked() {
                            new_sel = Some(Selection {
                                guild_id: Some(g.id),
                                channel_id: None,
                            });
                            fetch_guild_channels(rest.clone(), g.id);
                        }
                    }

                    // Folder expansion popup (grid of member servers, to the
                    // right of the bar) + context menus.
                    if let Some(idx) = open_folder {
                        render_folder_expansion(ui, app_state, &guilds, &config.guild_folders[idx], idx, rest.clone(), &mut new_sel);
                    }
                    if let Some(idx) = folder_ctx {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("guild_ctx"), GuildCtx::Folder(idx))
                        });
                    }
                    if let Some(gid) = guild_ctx {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("guild_ctx"), GuildCtx::Guild(gid))
                        });
                    }
                });

            // Guild/folder context menu (right-click) + folder persistence.
            render_guild_ctx_menu(ui, config, &guilds, &mut new_sel);

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

// ───────────────────────────── folders ─────────────────────────────

/// Which guild-bar context menu is open.
#[derive(Debug, Clone, Copy, PartialEq)]
enum GuildCtx {
    Guild(Snowflake),
    Folder(usize),
}

impl Default for GuildCtx {
    fn default() -> Self {
        GuildCtx::Folder(usize::MAX)
    }
}

/// The 2x2 mini-icon face of a folder button.
fn draw_folder_face(ui: &mut Ui, rect: Rect, members: &[&crate::model::Guild]) {
    let painter = ui.painter_at(rect);
    if members.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "+",
            egui::FontId::proportional(18.0),
            colors::TEXT_TERTIARY,
        );
        return;
    }
    if members.len() == 1 {
        let g = members[0];
        match g.icon_url() {
            Some(url) => {
                crate::ui::allocate_ui_at_rect(ui, rect, |ui| {
                    crate::image_loader::render_image(ui, &url, rect.width(), crate::image_loader::Shape::Rounded(34));
                });
            }
            None => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    g.initials(),
                    egui::FontId::proportional(14.0),
                    colors::TEXT_PRIMARY,
                );
            }
        }
        return;
    }
    // 2x2 grid of the first 4 members' icons (18px each, 3px gap).
    let cell = 20.0;
    let gap = 2.0;
    let grid = Rect::from_center_size(rect.center(), egui::vec2(cell * 2.0 + gap, cell * 2.0 + gap));
    for (i, g) in members.iter().take(4).enumerate() {
        let col = (i % 2) as f32;
        let row = (i / 2) as f32;
        let cell_rect = Rect::from_min_size(
            egui::pos2(grid.min.x + col * (cell + gap), grid.min.y + row * (cell + gap)),
            Vec2::splat(cell),
        );
        match g.icon_url() {
            Some(url) => {
                crate::ui::allocate_ui_at_rect(ui, cell_rect, |ui| {
                    crate::image_loader::render_image(ui, &url, cell, crate::image_loader::Shape::Rounded(30));
                });
            }
            None => {
                painter.text(
                    cell_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    g.initials(),
                    egui::FontId::proportional(10.0),
                    colors::TEXT_PRIMARY,
                );
            }
        }
    }
}

/// Expanded folder: a popup grid to the right of the guild bar with the
/// member server buttons (Discord's folder expansion).
fn render_folder_expansion(
    ui: &mut Ui,
    app_state: &AppState,
    guilds: &[crate::model::Guild],
    folder: &crate::config::GuildFolder,
    idx: usize,
    rest: std::sync::Arc<crate::rest::Http>,
    new_sel: &mut Option<Selection>,
) {
    let vp = ui.ctx().viewport_rect();
    let members: Vec<crate::model::Guild> = folder
        .guild_ids
        .iter()
        .filter_map(|id| id.parse::<u64>().ok())
        .filter_map(|id| guilds.iter().find(|g| g.id.0 == id).cloned())
        .collect();
    let h = (members.len() as f32 * 56.0 + 12.0).min(vp.height() - 24.0);
    let pos = egui::pos2(vp.min.x + 76.0, vp.min.y + 96.0);
    let area_rect = Rect::from_min_size(pos, egui::vec2(64.0, h));
    let frame = egui::Frame::new()
        .fill(colors::BG_FLOATING)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::same(6))
        .stroke(egui::Stroke::new(1.0, colors::BG_INPUT));
    let current_guild = app_state.selection_sync().guild_id;
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Area::new(egui::Id::new("folder_expand").with(idx))
        .order(egui::Order::Foreground)
        .fixed_pos(area_rect.min)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(52.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                for g in &members {
                    let is_active = current_guild == Some(g.id);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
                    let resp = resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(&g.name);
                    let radius = if is_active || resp.hovered() { 24.0 } else { 16.5 };
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, radius, colors::BG_ACCENT);
                    match g.icon_url() {
                        Some(url) => {
                            crate::ui::allocate_ui_at_rect(ui, rect, |ui| {
                                crate::image_loader::render_image(ui, &url, 48.0, crate::image_loader::Shape::Rounded(34));
                            });
                        }
                        None => {
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                g.initials(),
                                egui::FontId::proportional(14.0),
                                colors::TEXT_PRIMARY,
                            );
                        }
                    }
                    if resp.clicked() {
                        *new_sel = Some(Selection {
                            guild_id: Some(g.id),
                            channel_id: None,
                        });
                        fetch_guild_channels(rest.clone(), g.id);
                        close = true;
                    }
                }
            });
        });
    // Outside click closes the expansion.
    let outside = ui.input(|i| {
        i.pointer.button_clicked(egui::PointerButton::Primary)
            && i.pointer
                .interact_pos()
                .map(|p| !area_rect.contains(p) && p.x > vp.min.x + 72.0)
                .unwrap_or(false)
    });
    if close || outside {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("folder_open").with(idx), false));
    }
}

/// Right-click context menu for guilds and folders: add-to-folder,
/// remove-from-folder, rename, delete.
fn render_guild_ctx_menu(
    ui: &mut Ui,
    config: &mut crate::config::Config,
    guilds: &[crate::model::Guild],
    new_sel: &mut Option<Selection>,
) {
    let Some(ctx) = ui
        .ctx()
        .data(|d| d.get_temp::<GuildCtx>(egui::Id::new("guild_ctx")))
    else {
        return;
    };
    let vp = ui.ctx().viewport_rect();
    let anchor = ui
        .ctx()
        .pointer_interact_pos()
        .unwrap_or(egui::pos2(vp.min.x + 80.0, vp.min.y + 80.0));
    let pos = egui::pos2(
        (anchor.x + 12.0).clamp(vp.min.x + 4.0, vp.max.x - 240.0),
        anchor.y.clamp(vp.min.y + 4.0, vp.max.y - 220.0),
    );
    let w = 230.0;
    let close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let mut menu_h = 40.0;
    let items: Vec<String> = match ctx {
        GuildCtx::Guild(gid) => {
            let mut v = vec![];
            if let Some(folder_idx) = folder_of(config, gid) {
                let _ = folder_idx;
                v.push("__remove_from_folder".into());
            } else {
                v.push("__new_folder".into());
                for f in &config.guild_folders {
                    v.push(format!("__into:{}", f.name));
                }
            }
            v
        }
        GuildCtx::Folder(idx) => {
            let name = config
                .guild_folders
                .get(idx)
                .map(|f| f.name.clone())
                .unwrap_or_default();
            vec!["__rename_folder".into(), format!("__delete_folder:{}", name)]
        }
    };
    menu_h += items.len() as f32 * 30.0;
    let area_rect = Rect::from_min_size(pos, egui::vec2(w, menu_h));
    let frame = egui::Frame::new()
        .fill(colors::BG_FLOATING)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(6))
        .stroke(egui::Stroke::new(1.0, colors::BG_INPUT));
    let mut action: Option<String> = None;
    egui::Area::new(egui::Id::new("guild_ctx_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(w - 12.0);
                ui.vertical(|ui| {
                    let guild_name = match ctx {
                        GuildCtx::Guild(gid) => guilds
                            .iter()
                            .find(|g| g.id == gid)
                            .map(|g| g.name.clone())
                            .unwrap_or_default(),
                        GuildCtx::Folder(_) => String::new(),
                    };
                    if !guild_name.is_empty() {
                        ui.label(
                            egui::RichText::new(guild_name)
                                .size(12.0)
                                .strong()
                                .color(colors::TEXT_TERTIARY),
                        );
                        ui.add_space(4.0);
                    }
                    for item in &items {
                        let label = match item.as_str() {
                            "__new_folder" => "New folder with this server".to_string(),
                            "__remove_from_folder" => "Remove from folder".to_string(),
                            "__rename_folder" => "Rename folder".to_string(),
                            s if s.starts_with("__into:") => {
                                format!("Move to folder: {}", s.trim_start_matches("__into:"))
                            }
                            s if s.starts_with("__delete_folder:") => {
                                format!("Delete folder: {}", s.trim_start_matches("__delete_folder:"))
                            }
                            _ => item.clone(),
                        };
                        let resp = ui
                            .add(egui::Button::new(
                                egui::RichText::new(label).size(13.0).color(colors::TEXT_PRIMARY),
                            ))
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            action = Some(item.clone());
                        }
                    }
                });
            });
        });
    let outside = ui.input(|i| {
        i.pointer.button_clicked(egui::PointerButton::Primary)
            && i.pointer
                .interact_pos()
                .map(|p| !area_rect.contains(p))
                .unwrap_or(false)
    });
    if close || outside {
        ui.ctx()
            .data_mut(|d| d.remove_temp::<GuildCtx>(egui::Id::new("guild_ctx")));
    }
    if let Some(act) = action {
        match ctx {
            GuildCtx::Guild(gid) => match act.as_str() {
                "__new_folder" => {
                    let name = guilds
                        .iter()
                        .find(|g| g.id == gid)
                        .map(|g| format!("{} & more", g.initials()))
                        .unwrap_or_else(|| "Folder".into());
                    config.guild_folders.push(crate::config::GuildFolder {
                        name,
                        color: None,
                        guild_ids: vec![gid.0.to_string()],
                    });
                    let _ = config.save();
                    crate::ui::toast::success("Server added to a new folder.");
                }
                "__remove_from_folder" => {
                    if let Some(idx) = folder_of(config, gid) {
                        let mut folder = config.guild_folders[idx].clone();
                        folder.guild_ids.retain(|id| id != &gid.0.to_string());
                        if folder.guild_ids.is_empty() {
                            config.guild_folders.remove(idx);
                        } else {
                            config.guild_folders[idx] = folder;
                        }
                        let _ = config.save();
                        crate::ui::toast::info("Server removed from its folder.");
                    }
                }
                a if a.starts_with("__into:") => {
                    let fname = a.trim_start_matches("__into:").to_string();
                    if let Some(idx) = config.guild_folders.iter().position(|f| f.name == fname) {
                        config.guild_folders[idx]
                            .guild_ids
                            .push(gid.0.to_string());
                        let _ = config.save();
                        crate::ui::toast::success("Server moved to the folder.");
                    }
                }
                _ => {}
            },
            GuildCtx::Folder(idx) => match act.as_str() {
                "__rename_folder" => {
                    // Simple inline rename via the next popup pass.
                    ui.ctx().data_mut(|d| {
                        d.insert_temp::<String>(
                            egui::Id::new("rename_folder"),
                            config
                                .guild_folders
                                .get(idx)
                                .map(|f| f.name.clone())
                                .unwrap_or_default(),
                        )
                    });
                }
                a if a.starts_with("__delete_folder")
                    && idx < config.guild_folders.len() => {
                        config.guild_folders.remove(idx);
                        let _ = config.save();
                        crate::ui::toast::info("Folder deleted (servers stay in the bar).");
                    }
                _ => {}
            },
        }
        ui.ctx()
            .data_mut(|d| d.remove_temp::<GuildCtx>(egui::Id::new("guild_ctx")));
        let _ = new_sel;
    }

    // Folder rename popup.
    if let Some(mut name) = ui
        .ctx()
        .data(|d| d.get_temp::<String>(egui::Id::new("rename_folder")))
    {
        let mut done = false;
        egui::Area::new(egui::Id::new("rename_folder_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(pos.x, pos.y + 40.0))
            .interactable(true)
            .show(ui.ctx(), |ui| {
                let frame = egui::Frame::new()
                    .fill(colors::BG_FLOATING)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(8))
                    .stroke(egui::Stroke::new(1.0, colors::BG_INPUT));
                frame.show(ui, |ui| {
                    ui.set_width(200.0);
                    ui.label(
                        egui::RichText::new("Folder name").size(12.0).color(colors::TEXT_TERTIARY),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .desired_width(ui.available_width())
                            .font(egui::FontId::proportional(13.0)),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("Save").size(12.5).color(colors::TEXT_PRIMARY),
                            ))
                            .clicked()
                        {
                            done = true;
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("Cancel").size(12.5).color(colors::TEXT_TERTIARY),
                            ))
                            .clicked()
                        {
                            done = true;
                            name.clear();
                        }
                    });
                });
            });
        if done {
            if let GuildCtx::Folder(idx) = ctx {
                if !name.trim().is_empty() && idx < config.guild_folders.len() {
                    config.guild_folders[idx].name = name.trim().to_string();
                    let _ = config.save();
                }
            }
            ui.ctx()
                .data_mut(|d| d.remove_temp::<String>(egui::Id::new("rename_folder")));
        } else {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("rename_folder"), name));
        }
    }
}

/// Index of the folder containing this guild.
fn folder_of(config: &crate::config::Config, gid: Snowflake) -> Option<usize> {
    config
        .guild_folders
        .iter()
        .position(|f| f.guild_ids.contains(&gid.0.to_string()))
}
