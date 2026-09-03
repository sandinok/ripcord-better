//! Member list - right 240px panel. Rows built from the guild member cache
//! (GUILD_CREATE chunk or REST fallback), resolved through the user LRU,
//! with presence dots when presence data is available.

use egui::{Sense, Ui, Vec2};

use crate::colors;
use crate::state::{self, AppState};

pub fn render(ui: &mut Ui, app_state: &AppState) {
    let Some(guild_id) = app_state.selection_sync().guild_id else {
        return;
    };

    let frame = egui::Frame::new().fill(colors::BG_SIDEBAR);
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(ui.available_height());

        let mut member_ids = app_state.guild_member_ids(guild_id);

        // REST fallback when the gateway didn't deliver members (missing
        // GUILD_MEMBERS intent). Attempted once per guild.
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
                            Err(e) => {
                                tracing::debug!(error = %e, "member list unavailable (intent?)");
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
                section_label(ui, &format!("ONLINE - {}", online.len()));
                for (id, name, st) in &online {
                    render_member_row(ui, app_state, *id, name, st);
                }
                if !offline.is_empty() {
                    ui.add_space(8.0);
                    section_label(ui, &format!("OFFLINE - {}", offline.len()));
                    for (id, name, st) in &offline {
                        render_member_row(ui, app_state, *id, name, st);
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
            p.rect_filled(avatar_rect, 8.0, colors::BG_ACCENT);
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
}
