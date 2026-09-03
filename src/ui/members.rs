//! Member list - right 240px panel. Rows built from the guild member cache
//! (GUILD_MEMBERS_CHUNK from op 8, GUILD_CREATE, or REST fallback), resolved
//! through the user LRU, with live presence dots.
//!
//! Clicking a member opens a Discord-style user card: round avatar, status
//! dot, username, and their roles with colors.

use egui::{Rect, Sense, Ui, Vec2};

use crate::colors;
use crate::state::{self, AppState};

/// Which user the profile card is open for (shared with the chat panel via
/// egui temp data so clicks in either panel can open it).
pub const CARD_USER_ID: &str = "user_card_for";

pub fn render(ui: &mut Ui, app_state: &AppState) {
    let Some(guild_id) = app_state.selection_sync().guild_id else {
        return;
    };

    let frame = egui::Frame::new().fill(colors::BG_SIDEBAR);
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(ui.available_height());

        let mut member_ids = app_state.guild_member_ids(guild_id);

        // Ask the gateway for the full member list (op 8, with presences)
        // the first time this guild's panel is shown. The chunks that come
        // back merge into the cache; a second request is never needed.
        {
            let req_id = ui.id().with(("members_requested", guild_id.0));
            let requested = ui.ctx().data(|d| d.get_temp::<bool>(req_id)).unwrap_or(false);
            if !requested {
                ui.ctx().data_mut(|d| d.insert_temp(req_id, true));
                if crate::gateway::request_guild_members(guild_id) {
                    tracing::debug!(guild = %guild_id, "requested member list (op 8)");
                }
            }
        }

        // REST fallback when the gateway can't serve members (offline or
        // missing GUILD_MEMBERS intent). Attempted once per guild.
        if member_ids.is_empty() {
            if let Some(rest) = rest_handle() {
                let attempted_id = ui.id().with(("members_fetch", guild_id.0));
                let attempted = ui.ctx().data(|d| d.get_temp::<bool>(attempted_id)).unwrap_or(false);
                if !attempted {
                    ui.ctx().data_mut(|d| d.insert_temp(attempted_id, true));
                    let shared = state::global();
                    let gid = guild_id;
                    tokio::spawn(async move {
                        match rest.list_guild_members(gid, 100).await {
                            Ok(members) => {
                                let ids: Vec<crate::model::Snowflake> = members
                                    .iter()
                                    .filter_map(|m| m.user.as_ref().map(|u| u.id))
                                    .collect();
                                if let Some(s) = shared {
                                    for m in &members {
                                        if let Some(u) = &m.user {
                                            s.touch_user(u);
                                        }
                                    }
                                    if !ids.is_empty() {
                                        s.set_guild_members(gid, ids);
                                    }
                                }
                            }
                            Err(_) => {
                                // Members intent missing: the panel shows the
                                // REST-fetched subset instead of the live list.
                            }
                        }
                    });
                }
            }
        }

        // The bot itself is always a member (from /users/@me).
        if let Some(me) = app_state.current_user() {
            if !member_ids.contains(&me.id) {
                member_ids.push(me.id);
            }
        }

        let entries: Vec<(crate::model::Snowflake, String, String)> = member_ids
            .iter()
            .filter_map(|id| {
                let u = app_state.user(*id)?;
                let status = app_state.presence(*id).unwrap_or_else(|| "offline".into());
                Some((*id, u.display_name().to_string(), status))
            })
            .collect();

        let (online, offline): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|(_, _, st)| st != "offline");

        // Sort by name.
        let mut online = online;
        let mut offline = offline;
        online.sort_by_key(|(_, name, _)| name.to_lowercase());
        offline.sort_by_key(|(_, name, _)| name.to_lowercase());

        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                section_label(ui, &format!("ONLINE — {}", online.len()));
                for (id, name, st) in &online {
                    render_member_row(ui, app_state, *id, name, st, guild_id);
                }
                if !offline.is_empty() {
                    ui.add_space(8.0);
                    section_label(ui, &format!("OFFLINE — {}", offline.len()));
                    for (id, name, st) in &offline {
                        render_member_row(ui, app_state, *id, name, st, guild_id);
                    }
                }
                if online.is_empty() && offline.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new(if app_state.intents_limited() {
                                "Member list unavailable (enable the Server Members intent on the bot)."
                            } else {
                                "No members known yet."
                            })
                            .size(12.0)
                            .color(colors::TEXT_TERTIARY),
                        );
                    });
                }
            });
    });
}

fn rest_handle() -> Option<std::sync::Arc<crate::rest::Http>> {
    // The REST client is registered globally by the app at startup.
    crate::rest::global()
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new(text)
                .size(11.0)
                .color(colors::TEXT_TERTIARY)
                .strong(),
        );
    });
}

fn render_member_row(
    ui: &mut Ui,
    app_state: &AppState,
    id: crate::model::Snowflake,
    name: &str,
    status: &str,
    guild_id: crate::model::Snowflake,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
    let resp = ui
        .interact(rect, ui.id().with(("member", id.0)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter_at(rect).rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30));
    }
    let user = app_state.user(id);
    let avatar_url = user.as_ref().map(|u| u.avatar_url());
    let avatar_rect = egui::Rect::from_center_size(
        egui::pos2(rect.min.x + 30.0, rect.center().y),
        Vec2::splat(28.0),
    );
    crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
        if let Some(url) = avatar_url.as_deref() {
            crate::image_loader::render_avatar(ui, url, 28.0, name, Some(status));
        } else {
            let p = ui.painter_at(avatar_rect);
            p.rect_filled(avatar_rect, avatar_rect.width() / 2.0, colors::BG_ACCENT);
            p.text(
                avatar_rect.center(),
                egui::Align2::CENTER_CENTER,
                name.chars().next().unwrap_or('?').to_uppercase().to_string(),
                egui::FontId::proportional(12.0),
                colors::TEXT_PRIMARY,
            );
        }
    });
    let txt = if status == "offline" {
        colors::TEXT_MUTED
    } else {
        colors::TEXT_SECONDARY
    };
    ui.painter_at(rect).text(
        egui::pos2(rect.min.x + 52.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(14.0),
        txt,
    );
    // Click opens the user card (same widget the chat panel uses).
    if resp.clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(CARD_USER_ID), id));
    }
    // Hovering a member also refreshes their presence knowledge lazily:
    // the chunk request already covers it; nothing to do here.
    let _ = guild_id;
}

/// Render the user profile card popup (opened by clicking a member row or a
/// message author). Discord-style: banner strip, round avatar with status
/// dot, display name, @username, and colored roles.
pub fn render_user_card(ui: &mut Ui, app_state: &AppState, opened_at: Option<std::time::Instant>) {
    let Some(user_id) = ui.ctx().data(|d| d.get_temp::<crate::model::Snowflake>(egui::Id::new(CARD_USER_ID))) else {
        return;
    };
    let Some(user) = app_state.user(user_id) else {
        // Unknown user: close the card rather than showing an empty shell.
        ui.ctx().data_mut(|d| d.remove_temp::<crate::model::Snowflake>(egui::Id::new(CARD_USER_ID)));
        return;
    };

    // Grace window so the opening click cannot insta-close the card.
    let in_grace = opened_at
        .map(|t| t.elapsed() < std::time::Duration::from_millis(250))
        .unwrap_or(true);

    let status = app_state
        .presence(user_id)
        .or_else(|| {
            app_state
                .current_user()
                .filter(|me| me.id == user_id)
                .map(|_| app_state.own_status())
        })
        .unwrap_or_else(|| "offline".into());

    let sel = app_state.selection_sync();
    let guild = sel.guild_id.and_then(|g| app_state.guild_by_id(g));

    // Measure content first: card width 260, height depends on roles.
    let roles: Vec<(String, egui::Color32)> = guild
        .map(|g| {
            // The user's roles come from their member record in the cached
            // GUILD_CREATE / chunk payload; resolved against the guild's
            // role list for names + colors.
            let held: Vec<crate::model::Snowflake> = g
                .members
                .iter()
                .find(|m| m.user.as_ref().map(|u| u.id) == Some(user_id))
                .map(|m| m.roles.clone())
                .unwrap_or_default();
            held.iter()
                .filter_map(|rid| g.roles.iter().find(|r| r.id == *rid))
                .map(|r| {
                    let color = if r.color == 0 {
                        colors::TEXT_TERTIARY
                    } else {
                        role_color(r.color)
                    };
                    (r.name.clone(), color)
                })
                .collect()
        })
        .unwrap_or_default();

    let card_w = 260.0;
    let header_h = 60.0;
    // Reserve height: banner + avatar overhang + bottom breathing room,
    // plus the roles list when present. Only used to clamp the popup
    // position; the Area sizes itself to the real content.
    let roles_h = if roles.is_empty() { 0.0 } else { 30.0 + roles.len() as f32 * 20.0 };
    let card_h = header_h + 20.0 + 12.0 + roles_h;

    // Anchor near the top-left of the chat area, kept inside the viewport.
    let vp = ui.ctx().viewport_rect();
    let anchor = ui
        .ctx()
        .pointer_interact_pos()
        .unwrap_or(egui::pos2(vp.min.x + 340.0, vp.min.y + 80.0));
    let mut pos = egui::pos2(anchor.x - 40.0, anchor.y - 20.0);
    pos.x = pos.x.clamp(vp.min.x + 4.0, vp.max.x - card_w - 4.0);
    pos.y = pos.y.clamp(vp.min.y + 4.0, vp.max.y - card_h - 4.0);
    let card_rect = Rect::from_min_size(pos, egui::vec2(card_w, card_h));

    let mut close = false;
    let frame = egui::Frame::new()
        .fill(colors::BG_FLOATING)
        .corner_radius(10.0)
        .stroke(egui::Stroke::new(1.0, colors::BG_ACCENT))
        .inner_margin(egui::Margin::same(0));
    egui::Area::new(egui::Id::new("user_card").with(user_id.0))
        .order(egui::Order::Foreground)
        .fixed_pos(card_rect.min)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(card_w);
                // Concrete top-left of the card content: the cursor BEFORE
                // allocating (min_rect is empty until widgets exist, which is
                // why the banner used to paint at zero height).
                let top = ui.next_widget_position();
                // Accent banner, painted first so the avatar ring and name
                // block layer on top of it.
                ui.painter().rect_filled(
                    Rect::from_min_size(top, egui::vec2(card_w, header_h)),
                    8.0,
                    banner_color_for(user.accent_color, &user.id.0.to_string()),
                );
                // Reserve the banner block in the layout.
                ui.allocate_exact_size(Vec2::new(card_w, header_h), Sense::hover());
                // Avatar (circle, 72px) straddling the banner edge.
                let avatar_rect = Rect::from_center_size(
                    egui::pos2(top.x + 44.0, top.y + header_h),
                    Vec2::splat(72.0),
                );
                let ring = Rect::from_center_size(avatar_rect.center(), Vec2::splat(78.0));
                ui.painter_at(ring).rect_filled(ring, ring.width() / 2.0, colors::BG_FLOATING);
                crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
                    crate::image_loader::render_avatar(ui, &user.avatar_url(), 72.0, user.display_name(), Some(&status));
                });
                // Status dot sized for the bigger avatar.
                crate::image_loader::paint_status_dot(
                    &ui.painter_at(ring),
                    avatar_rect,
                    &status,
                    colors::BG_FLOATING,
                );

                // Name block to the right of the avatar.
                let name_rect = Rect::from_min_max(
                    egui::pos2(top.x + 92.0, top.y + header_h + 6.0),
                    egui::pos2(top.x + card_w - 12.0, top.y + header_h + 64.0),
                );
                crate::ui::allocate_ui_at_rect(ui, name_rect, |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(user.display_name())
                                .size(16.0)
                                .strong()
                                .color(colors::TEXT_PRIMARY),
                        );
                        ui.label(
                            egui::RichText::new(format!("@{}", user.username))
                                .size(12.5)
                                .color(colors::TEXT_TERTIARY),
                        );
                        if user.bot {
                            let (brect, _) = ui.allocate_exact_size(egui::vec2(32.0, 14.0), Sense::hover());
                            let p = ui.painter_at(brect);
                            p.rect_filled(brect, 3.0, colors::BLURPLE);
                            p.text(
                                brect.center(),
                                egui::Align2::CENTER_CENTER,
                                "BOT",
                                egui::FontId::proportional(9.0),
                                colors::TEXT_PRIMARY,
                            );
                        }
                    });
                });

                // Bottom block height (avatar overhang + margins): the 72px
                // avatar straddles the 60px banner, so 36px hang below it;
                // reserve that plus breathing room.
                ui.allocate_exact_size(Vec2::new(card_w, 46.0), Sense::hover());
                // Roles.
                if !roles.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new("ROLES")
                                .size(11.0)
                                .strong()
                                .color(colors::TEXT_TERTIARY),
                        );
                    });
                    for (name, color) in &roles {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(card_w - 24.0, 20.0), Sense::hover());
                        let pill = Rect::from_min_size(
                            egui::pos2(r.min.x + 16.0, r.min.y + 2.0),
                            egui::vec2(r.width() - 16.0, 16.0),
                        );
                        ui.painter_at(pill).rect_filled(pill, 4.0, color.gamma_multiply(0.18));
                        ui.painter_at(pill).text(
                            egui::pos2(pill.min.x + 8.0, pill.center().y),
                            egui::Align2::LEFT_CENTER,
                            name,
                            egui::FontId::proportional(12.0),
                            *color,
                        );
                    }
                }
            });
        });

    // Outside click (after grace) or Escape closes.
    let clicked_outside = ui.input(|i| {
        i.pointer.button_clicked(egui::PointerButton::Primary)
            && i.pointer
                .interact_pos()
                .map(|p| !card_rect.contains(p))
                .unwrap_or(false)
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) || (clicked_outside && !in_grace) {
        close = true;
    }
    let _ = &close; // read below
    if close {
        ui.ctx().data_mut(|d| d.remove_temp::<crate::model::Snowflake>(egui::Id::new(CARD_USER_ID)));
    }
}

/// Convert a Discord role color integer (0xRRGGBB) to Color32.
pub fn role_color(color: u32) -> egui::Color32 {
    egui::Color32::from_rgb(
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
    )
}

/// Deterministic banner color for users without an accent_color.
fn banner_color_for(accent: Option<u32>, seed: &str) -> egui::Color32 {
    if let Some(c) = accent.filter(|c| *c != 0) {
        return role_color(c);
    }
    let hash = seed.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    crate::colors::default_role_color_hex((hash % 18) as usize)
}
