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

/// Group-DM recipients panel (the members toggle in a DM header).
pub fn render_group_dm(ui: &mut Ui, app_state: &AppState) {
    let sel = app_state.selection_sync();
    let Some(cid) = sel.channel_id else { return };
    let Some(ch) = app_state.channel_by_id(cid) else { return };

    let frame = egui::Frame::new().fill(colors::BG_SIDEBAR);
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let recipients: Vec<crate::model::User> = if !ch.recipients.is_empty() {
                    ch.recipients.clone()
                } else {
                    ch.recipient_ids.iter().filter_map(|id| app_state.user(*id)).collect()
                };
                section_label(ui, &format!("MEMBERS — {}", recipients.len()));
                if recipients.is_empty() {
                    ui.label(
                        egui::RichText::new("No members yet.")
                            .size(12.0)
                            .color(colors::TEXT_TERTIARY),
                    );
                }
                for u in &recipients {
                    let row_h = 34.0;
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
                    let resp = ui
                        .interact(rect, ui.id().with(("gdm", u.id.0)), Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.hovered() {
                        ui.painter_at(rect).rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30));
                    }
                    let avatar_rect = egui::Rect::from_center_size(
                        egui::pos2(rect.min.x + 28.0, rect.center().y),
                        Vec2::splat(26.0),
                    );
                    crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
                        crate::image_loader::render_avatar(
                            ui,
                            &u.avatar_url(),
                            26.0,
                            u.display_name(),
                            Some(app_state.presence(u.id).as_deref().unwrap_or("offline")),
                        );
                    });
                    ui.painter_at(rect).text(
                        egui::pos2(rect.min.x + 48.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        u.display_name(),
                        egui::FontId::proportional(14.0),
                        colors::TEXT_SECONDARY,
                    );
                    if resp.clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(CARD_USER_ID), u.id));
                    }
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

#[allow(clippy::too_many_arguments)]
fn render_member_row(
    ui: &mut Ui,
    app_state: &AppState,
    id: crate::model::Snowflake,
    name: &str,
    status: &str,
    guild_id: crate::model::Snowflake,
) {
    // Activity second line ("Playing Rust"): rows grow when present.
    let activity = app_state.activity_line(id);
    let owner = app_state
        .guild_by_id(guild_id)
        .and_then(|g| g.owner_id)
        .map(|oid| oid == id)
        .unwrap_or(false);
    let user = app_state.user(id);
    let is_bot = user.as_ref().map(|u| u.bot).unwrap_or(false);
    // Name color: highest-ranked colored role.
    let name_color = user
        .as_ref()
        .map(|u| member_name_color(app_state, guild_id, u.id))
        .unwrap_or(colors::TEXT_SECONDARY);
    let height = if activity.is_some() { 44.0 } else { 36.0 };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let resp = ui
        .interact(rect, ui.id().with(("member", id.0)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter_at(rect).rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30));
    }
    let dim = status == "offline";
    let avatar_rect = egui::Rect::from_center_size(
        egui::pos2(rect.min.x + 28.0, rect.center().y),
        Vec2::splat(26.0),
    );
    crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
        let url = user.as_ref().map(|u| u.avatar_url()).unwrap_or_default();
        crate::image_loader::render_avatar(ui, &url, 26.0, name, Some(status));
    });
    let painter = ui.painter_at(rect);
    // Name (+ crown for the owner, APP/BOT chip for bots).
    let mut x = rect.min.x + 48.0;
    if owner {
        paint_crown(&painter, egui::pos2(x + 7.0, rect.min.y + if activity.is_some() { 12.0 } else { 10.0 }), 9.0);
        x += 17.0;
    }
    painter.text(
        egui::pos2(x, rect.min.y + if activity.is_some() { 15.0 } else { rect.center().y }),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(14.0),
        if dim { name_color.gamma_multiply(0.5) } else { name_color },
    );
    let name_w = crate::ui::emoji::label_width(ui, name, 14.0);
    if is_bot {
        let chip = egui::Rect::from_min_size(
            egui::pos2(x + name_w + 6.0, (if activity.is_some() { rect.min.y + 15.0 } else { rect.center().y }) - 7.0),
            egui::vec2(30.0, 14.0),
        );
        painter.rect_filled(chip, 3.0, colors::BLURPLE);
        painter.text(
            chip.center(),
            egui::Align2::CENTER_CENTER,
            "APP",
            egui::FontId::proportional(9.0),
            colors::TEXT_PRIMARY,
        );
    }
    if let Some(act) = &activity {
        painter.text(
            egui::pos2(rect.min.x + 48.0, rect.max.y - 10.0),
            egui::Align2::LEFT_CENTER,
            act,
            egui::FontId::proportional(11.0),
            colors::TEXT_TERTIARY.gamma_multiply(if dim { 0.6 } else { 1.0 }),
        );
    }
    // Click opens the user card (same widget the chat panel uses).
    if resp.clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(CARD_USER_ID), id));
    }
}

/// Owner crown (no Material icon for it): a small filled 5-point crown.
fn paint_crown(painter: &egui::Painter, center: egui::Pos2, r: f32) {
    let pts = [
        egui::pos2(center.x - r, center.y + r * 0.55),
        egui::pos2(center.x - r * 0.55, center.y - r * 0.5),
        egui::pos2(center.x - r * 0.18, center.y + r * 0.05),
        egui::pos2(center.x + r * 0.18, center.y - r * 0.5),
        egui::pos2(center.x + r * 0.55, center.y + r * 0.05),
        egui::pos2(center.x + r, center.y - r * 0.5),
        egui::pos2(center.x + r * 0.75, center.y + r * 0.55),
        egui::pos2(center.x - r * 0.75, center.y + r * 0.55),
    ];
    painter.add(egui::Shape::convex_polygon(
        pts.to_vec(),
        colors::STATUS_IDLE,
        egui::Stroke::NONE,
    ));
}

/// The member's name color from their highest colored role.
fn member_name_color(app_state: &AppState, guild_id: crate::model::Snowflake, user_id: crate::model::Snowflake) -> egui::Color32 {
    let Some(guild) = app_state.guild_by_id(guild_id) else {
        return colors::TEXT_SECONDARY;
    };
    let Some(member) = guild
        .members
        .iter()
        .find(|m| m.user.as_ref().map(|u| u.id) == Some(user_id))
    else {
        return colors::TEXT_SECONDARY;
    };
    let mut best: Option<(i32, egui::Color32)> = None;
    for rid in &member.roles {
        if let Some(role) = guild.roles.iter().find(|r| r.id == *rid) {
            if role.color != 0 {
                let better = match best {
                    Some((pos, _)) => role.position > pos,
                    None => true,
                };
                if better {
                    best = Some((role.position, role_color(role.color)));
                }
            }
        }
    }
    best.map(|(_, c)| c).unwrap_or(colors::TEXT_SECONDARY)
}

/// Render the full user profile popup (point 12): banner, big avatar
/// with status, badges, pronouns, bio with View Full Bio, roles,
/// activity, and member-since. Data comes from REST (bio/pronouns via
/// /users/{id}/profile when Discord allows it) + the cached guild data.
pub fn render_user_card(ui: &mut Ui, app_state: &AppState, opened_at: Option<std::time::Instant>) {
    let Some(user_id) = ui.ctx().data(|d| d.get_temp::<crate::model::Snowflake>(egui::Id::new(CARD_USER_ID))) else {
        return;
    };
    let Some(user) = app_state.user(user_id) else {
        ui.ctx().data_mut(|d| d.remove_temp::<crate::model::Snowflake>(egui::Id::new(CARD_USER_ID)));
        return;
    };

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

    // Kick the profile fetch (bio/pronouns) once per open.
    ensure_profile_fetched(app_state, user_id, sel.guild_id);

    // Roles (from the cached member record).
    let roles: Vec<(String, egui::Color32)> = guild.clone()
        .map(|g| {
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

    let profile = app_state.profile(user_id);
    let bio = profile.as_ref().and_then(|p| p.bio.clone()).filter(|b| !b.trim().is_empty());
    let pronouns = profile.as_ref().and_then(|p| p.pronouns.clone()).filter(|p| !p.trim().is_empty());
    let member_since = guild.clone()
        .and_then(|g| {
            g.members
                .iter()
                .find(|m| m.user.as_ref().map(|u| u.id) == Some(user_id))
                .and_then(|m| m.joined_at.clone())
        })
        .and_then(|ts| parse_iso_month_year(&ts));
    let activity = app_state.activity_line(user_id);
    let badges = user_flags(user.public_flags.unwrap_or(0));

    let card_w = 300.0;
    let banner_h = 90.0;

    // Height estimate for clamping (Area sizes itself to real content).
    let mut card_h = banner_h + 56.0 + 34.0;
    if !badges.is_empty() {
        card_h += 24.0;
    }
    if pronouns.is_some() {
        card_h += 18.0;
    }
    if bio.is_some() {
        card_h += 58.0;
    }
    if member_since.is_some() || activity.is_some() {
        card_h += 22.0;
    }
    if !roles.is_empty() {
        card_h += 30.0 + (roles.len() as f32 / 3.0).ceil() * 24.0;
    }
    card_h += 12.0;

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
    let mut bio_expanded = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(egui::Id::new("bio_expanded").with(user_id.0)))
        .unwrap_or(false);
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
                let top = ui.next_widget_position();

                // Banner: the user's real banner image when they have one,
                // else a deterministic accent gradient.
                let banner_rect = Rect::from_min_size(top, egui::vec2(card_w, banner_h));
                if let Some(banner_url) = user.banner_url() {
                    crate::image_loader::draw_cover_image(ui, banner_rect, &banner_url, 640, 180);
                } else {
                    let base = banner_color_for(user.accent_color, &user.id.0.to_string());
                    let painter = ui.painter_at(banner_rect);
                    painter.rect_filled(banner_rect, 8.0, base);
                    // Subtle animated shimmer (point 13, the tasteful kind).
                    let t = ui.input(|i| i.time) as f32;
                    let alpha = 0.06 + 0.04 * (t * 2.0).sin();
                    let sweep = (t * 0.25).fract();
                    let x = banner_rect.min.x + banner_rect.width() * sweep;
                    let stripe = Rect::from_min_max(
                        egui::pos2(x, banner_rect.min.y),
                        egui::pos2(x + 40.0, banner_rect.max.y),
                    );
                    painter.rect_filled(stripe, 0.0, egui::Color32::WHITE.gamma_multiply(alpha));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                }
                ui.allocate_exact_size(Vec2::new(card_w, banner_h), Sense::hover());

                // Avatar (80px) straddling the banner bottom edge.
                let avatar_rect = Rect::from_center_size(
                    egui::pos2(top.x + 52.0, top.y + banner_h),
                    Vec2::splat(80.0),
                );
                let ring = Rect::from_center_size(avatar_rect.center(), Vec2::splat(86.0));
                ui.painter_at(ring).rect_filled(ring, ring.width() / 2.0, colors::BG_FLOATING);
                crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
                    crate::image_loader::render_avatar(ui, &user.avatar_url(), 80.0, user.display_name(), Some(&status));
                });
                crate::image_loader::paint_status_dot(
                    &ui.painter_at(ring),
                    avatar_rect,
                    &status,
                    colors::BG_FLOATING,
                );

                // Name block right of the avatar.
                let name_rect = Rect::from_min_max(
                    egui::pos2(top.x + 100.0, top.y + banner_h + 2.0),
                    egui::pos2(top.x + card_w - 12.0, top.y + banner_h + 74.0),
                );
                crate::ui::allocate_ui_at_rect(ui, name_rect, |ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(user.display_name())
                                    .size(17.0)
                                    .strong()
                                    .color(colors::TEXT_PRIMARY),
                            );
                        });
                        ui.label(
                            egui::RichText::new(format!("@{}", user.username))
                                .size(12.5)
                                .color(colors::TEXT_TERTIARY),
                        );
                        if pronouns.is_some() {
                            // Filled after this block (needs ui again).
                        }
                    });
                });

                // Body block.
                ui.allocate_exact_size(Vec2::new(card_w, 12.0), Sense::hover());
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    // Pronouns + BOT chip + badges.
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(16.0);
                        if let Some(pr) = pronouns.as_deref() {
                            ui.label(
                                egui::RichText::new(format!("Pronouns: {pr}"))
                                    .size(12.0)
                                    .color(colors::TEXT_TERTIARY),
                            );
                        }
                        if user.bot {
                            let (brect, _) = ui.allocate_exact_size(egui::vec2(36.0, 14.0), Sense::hover());
                            let p = ui.painter_at(brect);
                            p.rect_filled(brect, 3.0, colors::BLURPLE);
                            p.text(
                                brect.center(),
                                egui::Align2::CENTER_CENTER,
                                "APP",
                                egui::FontId::proportional(9.0),
                                colors::TEXT_PRIMARY,
                            );
                        }
                        for badge in &badges {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(badge.len() as f32 * 6.5 + 14.0, 15.0), Sense::hover());
                            let p = ui.painter_at(r);
                            p.rect_filled(r, 3.0, colors::BG_ACCENT);
                            p.text(
                                r.center(),
                                egui::Align2::CENTER_CENTER,
                                badge,
                                egui::FontId::proportional(9.0),
                                colors::STATUS_IDLE,
                            );
                        }
                    });
                    // Bio with View Full Bio toggle.
                    if let Some(b) = bio.as_deref() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            let text = if bio_expanded {
                                b.to_string()
                            } else {
                                let short: String = b.chars().take(90).collect();
                                let suffix = if b.chars().count() > 90 { "..." } else { "" };
                                format!("{short}{suffix}")
                            };
                            ui.label(
                                egui::RichText::new(text)
                                    .size(12.5)
                                    .color(colors::TEXT_SECONDARY),
                            );
                        });
                        if b.chars().count() > 90 {
                            let resp = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(if bio_expanded { "Hide full bio" } else { "View Full Bio" })
                                        .size(12.0)
                                        .color(colors::TEXT_LINK),
                                )
                                .sense(egui::Sense::click()),
                            );
                            if resp.clicked() {
                                bio_expanded = !bio_expanded;
                            }
                        }
                    }
                    // Member since + activity.
                    ui.add_space(4.0);
                    if let Some(ms) = member_since.as_deref() {
                        ui.label(
                            egui::RichText::new(format!("Member since {ms}"))
                                .size(11.5)
                                .color(colors::TEXT_TERTIARY),
                        );
                    }
                    if let Some(act) = activity.as_deref() {
                        ui.label(
                            egui::RichText::new(act)
                                .size(11.5)
                                .color(colors::TEXT_TERTIARY),
                        );
                    }
                    // Roles as pills.
                    if !roles.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            ui.label(
                                egui::RichText::new("ROLES")
                                    .size(11.0)
                                    .strong()
                                    .color(colors::TEXT_TERTIARY),
                            );
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(16.0);
                            for (name, color) in &roles {
                                let pill_w = crate::ui::emoji::label_width(ui, name, 11.5) + 20.0;
                                let (r, resp) = ui.allocate_exact_size(egui::vec2(pill_w, 18.0), Sense::hover());
                                let _ = resp;
                                let p = ui.painter_at(r);
                                p.rect_filled(r, 9.0, color.gamma_multiply(0.18));
                                p.circle_filled(egui::pos2(r.min.x + 9.0, r.center().y), 4.0, *color);
                                p.text(
                                    egui::pos2(r.min.x + 17.0, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    name,
                                    egui::FontId::proportional(11.5),
                                    *color,
                                );
                            }
                        });
                    }
                    ui.add_space(10.0);
                });
            });
        });
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("bio_expanded").with(user_id.0), bio_expanded));

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
    if close {
        ui.ctx().data_mut(|d| d.remove_temp::<crate::model::Snowflake>(egui::Id::new(CARD_USER_ID)));
        ui.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::new("bio_expanded").with(user_id.0)));
    }
}

/// Fetch bio/pronouns once per user per session (bots can read this for
/// members of their guilds; strangers 403, which degrades gracefully).
fn ensure_profile_fetched(app_state: &AppState, user_id: crate::model::Snowflake, guild_id: Option<crate::model::Snowflake>) {
    if app_state.profile(user_id).is_some() {
        return;
    }
    // Reserve immediately so we don't fire every frame.
    app_state.set_profile(
        user_id,
        crate::model::UserProfile::default(),
    );
    let Some(rest) = crate::rest::global() else { return };
    tokio::spawn(async move {
        // /users/{id}/profile carries bio + pronouns when readable.
        if let Ok(p) = rest.get_user_profile(user_id).await {
            if let Some(s) = crate::state::global() {
                s.set_profile(user_id, p);
            }
        } else if let (Some(gid), Some(s)) = (guild_id, crate::state::global()) {
            // Fallback: the guild member endpoint still gives a fuller user
            // record (banner, flags) for members.
            if let Ok(m) = rest.get_guild_member(gid, user_id).await {
                if let Some(u) = &m.user {
                    s.touch_user(u);
                }
            }
            s.set_profile(user_id, crate::model::UserProfile::default());
        }
    });
}

/// "2024-08-13T..." -> "August 2024" (approx is fine for a profile card).
fn parse_iso_month_year(ts: &str) -> Option<String> {
    let year: i32 = ts.get(0..4)?.parse().ok()?;
    let month: u32 = ts.get(5..7)?.parse().ok()?;
    let names = [
        "January", "February", "March", "April", "May", "June",
        "July", "August", "September", "October", "November", "December",
    ];
    let name = names.get((month as usize).saturating_sub(1))?;
    Some(format!("{name} {year}"))
}

/// Discord public_flags bits -> short badge labels.
fn user_flags(flags: u64) -> Vec<&'static str> {
    let mut out = Vec::new();
    let has = |bit: u64| flags & (1 << bit) != 0;
    if has(0) {
        out.push("STAFF");
    }
    if has(1) {
        out.push("PARTNER");
    }
    if has(2) {
        out.push("HYPESQUAD");
    }
    if has(3) {
        out.push("BUG HUNTER");
    }
    if has(6) {
        out.push("BRAVERY");
    }
    if has(7) {
        out.push("BRILLIANCE");
    }
    if has(8) {
        out.push("BALANCE");
    }
    if has(9) {
        out.push("EARLY SUPPORTER");
    }
    if has(14) {
        out.push("BUG HUNTER 2");
    }
    if has(17) {
        out.push("VERIFIED BOT");
    }
    if has(18) {
        out.push("DEV");
    }
    if has(19) {
        out.push("MOD");
    }
    if has(22) {
        out.push("ACTIVE DEV");
    }
    out
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
