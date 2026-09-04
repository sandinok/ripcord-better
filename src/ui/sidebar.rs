//! Sidebar - 240px column. Channel tree with collapsible categories,
//! emoji-colored channel names, unread indicators, the Home/DM list with
//! a live filter, and the user box with a status picker popup.

use std::sync::Arc;

use egui::{Rect, Sense, Ui, Vec2};
use once_cell::sync::Lazy;
use dashmap::DashSet;

use crate::colors;
use crate::model::{Channel, ChannelType, Snowflake};
use crate::state::{self, AppState, Selection};
use crate::ui::emoji;

/// REST fetches currently in flight (prevents double-fetch storms).
static INFLIGHT_FETCH: Lazy<DashSet<u64>> = Lazy::new(DashSet::new);

pub fn render(
    ui: &mut Ui,
    app_state: &AppState,
    rest: Arc<crate::rest::Http>,
    config: &mut crate::config::Config,
) -> Option<Selection> {
    let mut new_sel: Option<Selection> = None;
    let sel = app_state.selection_sync();

    let rest_inner = rest.clone();
    let frame = egui::Frame::new().fill(colors::BG_SIDEBAR);
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        ui.vertical(|ui| {
            // ── Header (48px) ──
            let home = sel.guild_id.is_none();
            let header_text = if home {
                "Direct Messages".to_string()
            } else {
                app_state
                    .guild_by_id(sel.guild_id.unwrap())
                    .map(|g| g.name.clone())
                    .unwrap_or_else(|| "Unknown server".into())
            };
            // ── Header (48px, taller 135px when the guild has a banner) ──
            let banner = if home {
                None
            } else {
                app_state.guild_by_id(sel.guild_id.unwrap()).and_then(|g| g.banner_url())
            };
            let header_h = if banner.is_some() { 135.0 } else { 48.0 };
            let (header_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), header_h), Sense::hover());
            let painter = ui.painter_at(header_rect);
            painter.rect_filled(header_rect, 0.0, colors::BG_SIDEBAR);
            if let Some(url) = banner {
                // Banner image, cover-fit, with a gradient fade into the
                // sidebar so the channel list grows out of it naturally.
                crate::image_loader::draw_cover_image(ui, header_rect, &url, 600, 240);
                // Bottom fade into the sidebar: stacked translucent slices
                // (egui has no gradient primitive; 8 slices read as smooth).
                const BG: (u8, u8, u8) = (0x2B, 0x2D, 0x31);
                for i in 0..8 {
                    let a = (i as f32 / 7.0 * 255.0) as u8;
                    let h = 44.0 / 8.0;
                    let r = Rect::from_min_size(
                        egui::pos2(header_rect.min.x, header_rect.max.y - 44.0 + i as f32 * h),
                        egui::vec2(header_rect.width(), h + 1.0),
                    );
                    ui.painter().rect_filled(
                        r,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(BG.0, BG.1, BG.2, a),
                    );
                }
            }
            let text_y = if header_h > 48.0 {
                header_rect.max.y - 24.0
            } else {
                header_rect.center().y
            };
            painter.text(
                egui::pos2(header_rect.min.x + 16.0, text_y),
                egui::Align2::LEFT_CENTER,
                &header_text,
                egui::FontId::proportional(15.0),
                colors::TEXT_HEADER,
            );
            crate::icons::draw(
                ui.painter(),
                "expand_more",
                egui::pos2(header_rect.max.x - 22.0, text_y),
                18.0,
                colors::TEXT_TERTIARY,
            );

            // ── Body ──
            // The scroll area must leave room for the pinned user box
            // (54px) at the bottom; measure from the panel clip rect. We
            // pass an absolute bottom limit so per-body headers (the DM
            // filter box, category rows) are accounted for naturally.
            let body_bottom = ui.max_rect().bottom() - 58.0;
            if home {
                render_dm_list(ui, app_state, rest_inner, &mut new_sel, config, body_bottom);
            } else {
                let gid = sel.guild_id.unwrap();
                // Auto-select the first channel of a newly-entered guild
                // (runs until the channel list arrives + a channel is set).
                if sel.channel_id.is_none() {
                    auto_select_first_channel(app_state);
                }
                render_channel_tree(ui, app_state, gid, &mut new_sel, body_bottom, config);
            }

            // ── User box (pinned at the bottom by the reserved body height) ──
            ui.allocate_space(ui.available_size().min(egui::vec2(0.0, 0.0)));
            render_user_box(ui, app_state);
        });
    });

    // Belt-and-suspenders: if a channel is selected but its history was
    // never fetched, fetch it now (covers selections made outside clicks).
    if let Some(cid) = sel.channel_id {
        if app_state.channel_by_id(cid).map(|c| c.is_text_like()).unwrap_or(false)
            && !app_state.is_fetched(cid)
        {
            if let Some(shared) = state::global() {
                fetch_channel_messages(&rest, &shared, cid);
            }
        }
    }

    new_sel
}

// ───────────────────────────── channel tree ─────────────────────────────

fn render_channel_tree(
    ui: &mut Ui,
    app_state: &AppState,
    guild_id: Snowflake,
    new_sel: &mut Option<Selection>,
    body_bottom: f32,
    config: &mut crate::config::Config,
) {
    let channels = app_state.channels_for_guild(guild_id);
    if channels.is_empty() {
        let scroll_height = (body_bottom - ui.next_widget_position().y).max(48.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show(ui, |ui| {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("Loading channels...")
                        .size(13.0)
                        .color(colors::TEXT_TERTIARY),
                );
            });
        });
        return;
    }

    let selected = app_state.selection_sync().channel_id;
    let scroll_height = (body_bottom - ui.next_widget_position().y).max(48.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(scroll_height)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.add_space(8.0);

            // Category-less channels first (text then voice, by position).
            let no_category: Vec<&Channel> = channels
                .iter()
                .filter(|c| c.parent_id.is_none() && !matches!(c.kind, ChannelType::Category))
                .filter(|c| c.is_text_like() || c.is_voice_like())
                .collect();
            render_channel_section(ui, app_state, &no_category, selected, new_sel, false, guild_id);

            // Categories with children.
            let mut categories: Vec<&Channel> = channels
                .iter()
                .filter(|c| matches!(c.kind, ChannelType::Category))
                .collect();
            categories.sort_by_key(|c| c.position.unwrap_or(0));
            for cat in categories {
                let children: Vec<&Channel> = channels
                    .iter()
                    .filter(|c| c.parent_id == Some(cat.id))
                    .filter(|c| c.is_text_like() || c.is_voice_like())
                    .collect();
                if children.is_empty() {
                    continue;
                }
                render_category_header(ui, app_state, cat, &children, selected, new_sel, guild_id);
                render_channel_section(ui, app_state, &children, selected, new_sel, true, guild_id);
                ui.add_space(6.0);
            }
            ui.add_space(12.0);
        });

    // Sidebar popups (invite / channel settings / new channel / events /
    // boosts) render above everything once per frame.
    render_sidebar_popup(ui, app_state, Some(guild_id));
    let _ = config;
}

fn render_channel_section(
    ui: &mut Ui,
    app_state: &AppState,
    channels: &[&Channel],
    selected: Option<Snowflake>,
    new_sel: &mut Option<Selection>,
    indent: bool,
    guild_id: Snowflake,
) {
    let mut sorted: Vec<&&Channel> = channels.iter().collect();
    sorted.sort_by_key(|c| c.position.unwrap_or(0));
    for c in sorted {
        render_channel_row(ui, app_state, c, selected == Some(c.id), new_sel, indent);
        // Voice channels list connected members right under the row
        // (real data from VOICE_STATE_UPDATE).
        if c.is_voice_like() {
            let users = app_state.voice_channel_users(guild_id, c.id);
            for uid in users {
                let user = app_state.user(uid);
                let name = user
                    .as_ref()
                    .map(|u| u.display_name().to_string())
                    .unwrap_or_else(|| "member".into());
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
                let _resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                let painter = ui.painter_at(rect);
                let avatar_rect = Rect::from_center_size(
                    egui::pos2(rect.min.x + left_pad_for(indent) + 20.0, rect.center().y),
                    Vec2::splat(18.0),
                );
                let _ = avatar_rect;
                crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
                    let url = user.as_ref().map(|u| u.avatar_url()).unwrap_or_default();
                    crate::image_loader::render_avatar(ui, &url, 18.0, &name, None);
                });
                painter.text(
                    egui::pos2(rect.min.x + left_pad_for(indent) + 36.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(12.0),
                    colors::TEXT_TERTIARY,
                );
            }
        }
    }
}

fn left_pad_for(indent: bool) -> f32 {
    if indent { 30.0 } else { 16.0 }
}

fn render_category_header(
    ui: &mut Ui,
    app_state: &AppState,
    cat: &Channel,
    children: &[&Channel],
    _selected: Option<Snowflake>,
    new_sel: &mut Option<Selection>,
    guild_id: Snowflake,
) {
    let _ = (new_sel, guild_id, children);
    // Any child active or unread keeps the category visually open.
    let id = ui.id().with(("cat", cat.id.0));
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
    let resp = ui
        .interact(rect, id.with("hdr"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        open = !open;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, open));

    let painter = ui.painter_at(rect);
    if resp.hovered() {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30));
    }
    // Chevron: filled triangle, down when open, right when closed.
    let chev_icon = if open { "arrow_drop_down" } else { "arrow_right" };
    crate::icons::draw(
        &painter,
        chev_icon,
        egui::pos2(rect.min.x + 26.0, rect.center().y),
        18.0,
        colors::TEXT_TERTIARY,
    );
    let label = cat.name.to_uppercase();
    let has_unread = children.iter().any(|c| app_state.unread_count(c.id) > 0);
    painter.text(
        egui::pos2(rect.min.x + 40.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.0),
        if has_unread { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY },
    );
    // The "+" affordance on the right: creates a channel inside this
    // category (real POST /guilds/{gid}/channels with parent_id).
    let plus_rect = Rect::from_center_size(
        egui::pos2(rect.max.x - 20.0, rect.center().y),
        Vec2::splat(22.0),
    );
    let plus_resp = ui
        .interact(plus_rect, id.with("plus"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("Create channel in {}", cat.name));
    let p = ui.painter_at(plus_rect);
    p.rect_filled(plus_rect, 4.0, colors::BG_SIDEBAR);
    let pc = if plus_resp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY };
    p.line_segment(
        [egui::pos2(plus_rect.center().x - 5.0, plus_rect.center().y), egui::pos2(plus_rect.center().x + 5.0, plus_rect.center().y)],
        egui::Stroke::new(1.5, pc),
    );
    p.line_segment(
        [egui::pos2(plus_rect.center().x, plus_rect.center().y - 5.0), egui::pos2(plus_rect.center().x, plus_rect.center().y + 5.0)],
        egui::Stroke::new(1.5, pc),
    );
    if plus_resp.clicked() {
        open_sidebar_popup(ui, cat.id, SidePopup::NewChannel { parent: Some(cat.id) });
    }
}

fn render_channel_row(
    ui: &mut Ui,
    app_state: &AppState,
    c: &Channel,
    is_active: bool,
    new_sel: &mut Option<Selection>,
    indent: bool,
) {
    let row_h = 32.0;
    let left_pad = if indent { 30.0 } else { 16.0 };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
    let response = ui
        .interact(rect, ui.id().with(("channel", c.id.0)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect);

    let unread = app_state.unread_count(c.id);
    let mentions = app_state.mention_count(c.id);

    // Hover animates in/out (0.30 target over 90ms) instead of snapping.
    let hover_t = ui.ctx().animate_value_with_time(
        ui.id().with(("channel_hover", c.id.0)),
        if response.hovered() { 1.0 } else { 0.0 },
        0.09,
    );
    if is_active {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.55));
    } else if hover_t > 0.01 {
        let a = 0.16 * (unread > 0) as i32 as f32 + 0.30 * hover_t;
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(a));
    } else if unread > 0 {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.16));
    }

    // Unread dot at the far left.
    if !is_active && unread > 0 {
        painter.circle_filled(
            egui::pos2(rect.min.x + 7.0, rect.center().y),
            3.0,
            colors::TEXT_PRIMARY,
        );
    }

    let txt_color = if is_active || unread > 0 {
        colors::TEXT_PRIMARY
    } else {
        colors::TEXT_TERTIARY
    };

    // Channel-type icon (hash / speaker / DM).
    let icon = match c.kind {
        ChannelType::Voice | ChannelType::StageVoice => "volume_up",
        _ => "tag",
    };
    crate::icons::draw(
        &painter,
        icon,
        egui::pos2(rect.min.x + left_pad + 10.0, rect.center().y),
        20.0,
        txt_color,
    );

    // Name (emoji-aware).
    let name_rect = Rect::from_min_max(
        egui::pos2(rect.min.x + left_pad + 26.0, rect.min.y + 4.0),
        egui::pos2(rect.max.x - 40.0, rect.max.y - 4.0),
    );
    crate::ui::allocate_ui_at_rect(ui, name_rect, |ui| {
        ui.horizontal_centered(|ui| {
            emoji::render_label(ui, &c.display_name(), 14.0, txt_color);
        });
    });

    // Mention badge on the right.
    if mentions > 0 {
        let badge_r = if mentions > 9 { 9.5 } else { 8.0 };
        let center = egui::pos2(rect.max.x - 16.0, rect.center().y);
        painter.circle_filled(center, badge_r, colors::RED);
        let badge_text = if mentions > 99 { "99+".to_string() } else { mentions.to_string() };
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            &badge_text,
            egui::FontId::proportional(10.0),
            colors::TEXT_PRIMARY,
        );
    }

    // Hover actions on the right: invite + channel settings (text-like
    // channels only, Discord's exact affordance).
    if !c.is_voice_like() && (response.hovered() || sidebar_popup_for(ui, c.id)) {
        let invite_r = Rect::from_center_size(
            egui::pos2(rect.max.x - 34.0, rect.center().y),
            Vec2::splat(24.0),
        );
        let set_r = Rect::from_center_size(
            egui::pos2(rect.max.x - 62.0, rect.center().y),
            Vec2::splat(24.0),
        );
        let bg = colors::BG_SIDEBAR;
        for (r, id_key, icon, tooltip) in [
            (invite_r, "ch_invite", "person_add", "Create invite link"),
            (set_r, "ch_settings", "settings", "Channel settings"),
        ] {
            let resp = ui
                .interact(r, ui.id().with((id_key, c.id.0)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(tooltip);
            let painter2 = ui.painter_at(r);
            painter2.rect_filled(r, 4.0, bg);
            let ic = if resp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY };
            crate::icons::draw(&painter2, icon, r.center(), 16.0, ic);
            if resp.clicked() {
                open_sidebar_popup(ui, c.id, if id_key == "ch_invite" { SidePopup::Invite } else { SidePopup::Settings });
            }
        }
        // Dim the mention badge spot when actions overlap.
        let _ = mentions;
    }

    if response.clicked() {
        *new_sel = Some(Selection {
            guild_id: c.guild_id,
            channel_id: Some(c.id),
        });
    }
}

/// Which sidebar popup is open (invite / channel settings / new channel /
/// events / boosts / dm context).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum SidePopup {
    #[default]
    Invite,
    Settings,
    NewChannel { parent: Option<Snowflake> },
    Events,
    Boosts,
    DmContext { dm: Snowflake },
}


const SIDEBAR_POPUP: &str = "sidebar_popup"; // (channel id u64, SidePopup)

fn open_sidebar_popup(ui: &Ui, channel_id: Snowflake, popup: SidePopup) {
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new(SIDEBAR_POPUP),
            (channel_id.0, popup),
        )
    });
}

fn sidebar_popup_for(ui: &Ui, channel_id: Snowflake) -> bool {
    ui.ctx()
        .data(|d| d.get_temp::<(u64, SidePopup)>(egui::Id::new(SIDEBAR_POPUP)))
        .map(|(id, _)| id == channel_id.0)
        .unwrap_or(false)
}

/// Render the active sidebar popup (anchored, foreground). Called once
/// per frame from the sidebar render.
fn render_sidebar_popup(ui: &mut Ui, app_state: &AppState, gid: Option<Snowflake>) {
    let Some((chan_u64, popup)) = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, SidePopup)>(egui::Id::new(SIDEBAR_POPUP)))
    else {
        return;
    };
    let chan = crate::model::Snowflake(chan_u64);
    let vp = ui.ctx().viewport_rect();
    let pos = ui
        .ctx()
        .pointer_interact_pos()
        .map(|p| egui::pos2(p.x.clamp(vp.min.x + 4.0, vp.max.x - 350.0), p.y.clamp(vp.min.y + 4.0, vp.max.y - 300.0)))
        .unwrap_or(egui::pos2(vp.min.x + 260.0, vp.min.y + 60.0));
    let w = 320.0;
    let close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let just_opened = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(egui::Id::new(SIDEBAR_POPUP).with("grace")))
        .is_none();
    if just_opened {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new(SIDEBAR_POPUP).with("gr"), true));
    }
    let frame = egui::Frame::new()
        .fill(colors::BG_FLOATING)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(12))
        .stroke(egui::Stroke::new(1.0, colors::BG_INPUT));
    egui::Area::new(egui::Id::new("side_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(w - 24.0);
                match popup {
                    SidePopup::Invite => {
                        ui.label(
                            egui::RichText::new("Invite people")
                                .size(14.0)
                                .strong()
                                .color(colors::TEXT_PRIMARY),
                        );
                        ui.add_space(6.0);
                        // Fire the invite creation once.
                        invite_flow(ui, chan);
                    }
                    SidePopup::Settings => {
                        channel_settings_ui(ui, app_state, chan);
                    }
                    SidePopup::NewChannel { parent } => {
                        new_channel_ui(ui, gid, parent);
                    }
                    SidePopup::Events => {
                        events_ui(ui, app_state, gid);
                    }
                    SidePopup::Boosts => {
                        boosts_ui(ui, app_state, gid);
                    }
                    SidePopup::DmContext { dm } => {
                        dm_context_ui(ui, app_state, dm);
                    }
                }
            });
        });
    let outside = !just_opened
        && ui.input(|i| {
            i.pointer.button_clicked(egui::PointerButton::Primary)
                && i.pointer
                    .interact_pos()
                    .map(|p| p.x < pos.x || p.x > pos.x + w || p.y < pos.y || p.y > pos.y + 320.0)
                    .unwrap_or(false)
        });
    if close || outside {
        ui.ctx().data_mut(|d| d.remove_temp::<(u64, SidePopup)>(egui::Id::new(SIDEBAR_POPUP)));
        ui.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::new(SIDEBAR_POPUP).with("grace")));
        ui.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::new(SIDEBAR_POPUP).with("gr")));
    }
}

/// Created invite links per channel (session cache).
static INVITES: Lazy<dashmap::DashMap<u64, String>> = Lazy::new(dashmap::DashMap::new);

/// Create-invite flow: one POST, the link shows with a Copy button.
fn invite_flow(ui: &mut Ui, chan: Snowflake) {
    let url = INVITES.get(&chan.0).map(|v| v.clone());
    let url = match url {
        Some(u) => u,
        None => {
            let fired = ui
                .ctx()
                .data(|d| d.get_temp::<bool>(egui::Id::new("invite_fired").with(chan.0)))
                .unwrap_or(false);
            if !fired {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("invite_fired").with(chan.0), true));
                if let Some(rest) = crate::rest::global() {
                    tokio::spawn(async move {
                        match rest.create_invite(chan).await {
                            Ok(inv) => {
                                INVITES.insert(chan.0, inv.url());
                                if let Some(s) = crate::state::global() {
                                    let _ = s.event_sender().send(
                                        crate::gateway::events::Event::RepaintRequested,
                                    );
                                }
                            }
                            Err(e) => {
                                crate::ui::toast::error(format!("Could not create an invite: {e}"));
                            }
                        }
                    });
                }
            }
            ui.label(
                egui::RichText::new("Creating invite...")
                    .size(12.5)
                    .color(colors::TEXT_TERTIARY),
            );
            return;
        }
    };
    ui.label(
        egui::RichText::new("Share this link - it expires in 24 hours")
            .size(12.0)
            .color(colors::TEXT_TERTIARY),
    );
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::singleline(&mut url.clone())
            .desired_width(ui.available_width())
            .font(egui::FontId::proportional(12.0)),
    );
    if ui
        .add(egui::Button::new(
            egui::RichText::new("Copy link").size(12.5).color(colors::TEXT_PRIMARY),
        ))
        .clicked()
    {
        ui.ctx().copy_text(url.clone());
        crate::ui::toast::success("Invite link copied.");
    }
}

/// Rename / retopic / delete a channel (real PATCH + DELETE).
fn channel_settings_ui(ui: &mut Ui, app_state: &AppState, chan: Snowflake) {
    let Some(ch) = app_state.channel_by_id(chan) else { return };
    ui.label(
        egui::RichText::new(format!("Channel settings - #{}", ch.name))
            .size(14.0)
            .strong()
            .color(colors::TEXT_PRIMARY),
    );
    ui.add_space(8.0);
    let key = egui::Id::new("ch_rename").with(chan.0);
    let mut name: String = ui.ctx().data(|d| d.get_temp(key)).unwrap_or_else(|| ch.name.clone());
    ui.label(egui::RichText::new("Channel name").size(12.0).color(colors::TEXT_TERTIARY));
    ui.add(
        egui::TextEdit::singleline(&mut name)
            .desired_width(ui.available_width())
            .font(egui::FontId::proportional(13.0)),
    );
    ui.ctx().data_mut(|d| d.insert_temp(key, name.clone()));
    let topic_key = egui::Id::new("ch_topic").with(chan.0);
    let mut topic: String = ui
        .ctx()
        .data(|d| d.get_temp(topic_key))
        .unwrap_or_else(|| ch.topic.clone().unwrap_or_default());
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Topic").size(12.0).color(colors::TEXT_TERTIARY));
    ui.add(
        egui::TextEdit::singleline(&mut topic)
            .desired_width(ui.available_width())
            .font(egui::FontId::proportional(13.0)),
    );
    ui.ctx().data_mut(|d| d.insert_temp(topic_key, topic.clone()));
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let can = !name.trim().is_empty() && name.trim() != ch.name;
        let save = ui.add(
            egui::Button::new(egui::RichText::new("Save changes").size(12.5).color(colors::TEXT_PRIMARY))
                .fill(if can { colors::BLURPLE } else { colors::BG_ACCENT }),
        );
        if save.clicked() && can {
            if let Some(rest) = crate::rest::global() {
                let name = name.trim().to_string();
                let topic = topic.trim().to_string();
                tokio::spawn(async move {
                    match rest
                        .modify_channel(chan, Some(&name), if topic.is_empty() { None } else { Some(&topic) })
                        .await
                    {
                        Ok(_) => crate::ui::toast::success("Channel updated."),
                        Err(e) => crate::ui::toast::error(format!("Rename failed: {e}")),
                    }
                });
            }
        }
        // Delete (confirm step).
        let del_key = egui::Id::new("ch_del_confirm").with(chan.0);
        let mut confirming = ui.ctx().data(|d| d.get_temp::<bool>(del_key)).unwrap_or(false);
        let del = ui.add(
            egui::Button::new(egui::RichText::new(if confirming { "Really delete?" } else { "Delete channel" }).size(12.5).color(colors::TEXT_PRIMARY))
                .fill(colors::RED.gamma_multiply(if confirming { 1.0 } else { 0.6 })),
        );
        if del.clicked() {
            if confirming {
                if let Some(rest) = crate::rest::global() {
                    tokio::spawn(async move {
                        match rest.delete_channel(chan).await {
                            Ok(_) => crate::ui::toast::success("Channel deleted."),
                            Err(e) => crate::ui::toast::error(format!("Delete failed: {e}")),
                        }
                    });
                }
                ui.ctx().data_mut(|d| d.insert_temp(del_key, false));
            } else {
                ui.ctx().data_mut(|d| d.insert_temp(del_key, true));
            }
        }
        confirming = !confirming && false;
        let _ = confirming;
    });
}

/// Create a channel inside a category (the category "+" button).
fn new_channel_ui(ui: &mut Ui, gid: Option<Snowflake>, parent: Option<Snowflake>) {
    let Some(gid) = gid else { return };
    ui.label(
        egui::RichText::new("Create channel")
            .size(14.0)
            .strong()
            .color(colors::TEXT_PRIMARY),
    );
    ui.add_space(8.0);
    let key = egui::Id::new("new_ch_name").with(gid.0).with(parent.map(|p| p.0).unwrap_or(0));
    let mut name: String = ui.ctx().data(|d| d.get_temp(key)).unwrap_or_default();
    ui.label(egui::RichText::new("Channel name").size(12.0).color(colors::TEXT_TERTIARY));
    ui.add(
        egui::TextEdit::singleline(&mut name)
            .desired_width(ui.available_width())
            .font(egui::FontId::proportional(13.0)),
    );
    ui.ctx().data_mut(|d| d.insert_temp(key, name.clone()));
    let kind_key = key.with("kind");
    let mut voice: bool = ui.ctx().data(|d| d.get_temp::<bool>(kind_key)).unwrap_or(false);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let txt = ui.selectable_value(&mut voice, false, "Text");
        if txt.clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(kind_key, false));
        }
        let vc = ui.selectable_value(&mut voice, true, "Voice");
        if vc.clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(kind_key, true));
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(kind_key, voice));
    ui.add_space(6.0);
    let can = !name.trim().is_empty();
    let btn = ui.add(
        egui::Button::new(egui::RichText::new("Create channel").size(12.5).color(colors::TEXT_PRIMARY))
            .fill(if can { colors::BLURPLE } else { colors::BG_ACCENT }),
    );
    if btn.clicked() && can {
        if let Some(rest) = crate::rest::global() {
            let name = name.trim().to_string();
            let kind = if voice { 2u8 } else { 0u8 };
            tokio::spawn(async move {
                match rest.create_channel(gid, &name, kind, parent, None).await {
                    Ok(_) => crate::ui::toast::success(format!("Created #{}", name)),
                    Err(e) => crate::ui::toast::error(format!("Create failed: {e}")),
                }
            });
        }
    }
}

/// "2026-09-01T..." -> "September 2026" (approx for event rows).
fn parse_iso_month_year(ts: &str) -> String {
    let year: i32 = ts.get(0..4).and_then(|y| y.parse().ok()).unwrap_or(0);
    let month: u32 = ts.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(0);
    let names = [
        "January", "February", "March", "April", "May", "June",
        "July", "August", "September", "October", "November", "December",
    ];
    let name = names.get((month as usize).saturating_sub(1)).copied().unwrap_or("?");
    format!("{name} {year}")
}

/// Events popup: the guild's scheduled events with attendee counts.
fn events_ui(ui: &mut Ui, app_state: &AppState, gid: Option<Snowflake>) {
    let Some(gid) = gid else { return };
    ui.label(
        egui::RichText::new("Events")
            .size(14.0)
            .strong()
            .color(colors::TEXT_PRIMARY),
    );
    ui.add_space(6.0);
    // Fire the fetch once per open.
    let fired = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(egui::Id::new("events_fired").with(gid.0)))
        .unwrap_or(false);
    if !fired {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("events_fired").with(gid.0), true));
        if let Some(rest) = crate::rest::global() {
            tokio::spawn(async move {
                if let Ok(events) = rest.get_scheduled_events(gid).await {
                    if let Some(s) = crate::state::global() {
                        s.set_events(gid, events);
                    }
                }
            });
        }
    }
    let events = app_state.events_for(gid);
    if events.is_empty() {
        ui.label(
            egui::RichText::new("No scheduled events on this server. Create one in Discord when you need it.")
                .size(12.0)
                .color(colors::TEXT_TERTIARY),
        );
        return;
    }
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for ev in &events {
            ui.horizontal(|ui| {
                crate::icons::draw(ui.painter(), "event", egui::pos2(ui.next_widget_position().x + 10.0, ui.next_widget_position().y + 12.0), 16.0, colors::TEXT_TERTIARY);
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&ev.name)
                            .size(13.0)
                            .strong()
                            .color(colors::TEXT_PRIMARY),
                    );
                    let when = ev.start.as_deref().map(parse_iso_month_year).unwrap_or_default();
                    let count = ev.user_count.map(|c| format!(" - {c} interested")).unwrap_or_default();
                    ui.label(
                        egui::RichText::new(format!("{when}{count}"))
                            .size(11.5)
                            .color(colors::TEXT_TERTIARY),
                    );
                });
            });
        }
    });
}

/// Server Boosts popup: real tier + boost count from the guild object.
fn boosts_ui(ui: &mut Ui, app_state: &AppState, gid: Option<Snowflake>) {
    let Some(gid) = gid else { return };
    let g = app_state.guild_by_id(gid);
    ui.label(
        egui::RichText::new("Server Boosts")
            .size(14.0)
            .strong()
            .color(colors::TEXT_PRIMARY),
    );
    ui.add_space(6.0);
    let tier = g.as_ref().and_then(|g| g.premium_tier).unwrap_or(0);
    let count = g
        .as_ref()
        .and_then(|g| g.premium_subscription_count)
        .unwrap_or(0);
    let tier_label = match tier {
        1 => "Tier 1",
        2 => "Tier 2",
        3 => "Tier 3",
        _ => "No tier yet",
    };
    // Gem row.
    for _ in 0..tier.min(3) {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        crate::icons::draw(ui.painter(), "diamond", r.center(), 18.0, colors::BLURPLE);
    }
    ui.label(
        egui::RichText::new(format!("{tier_label} - {count} boost{}", if count == 1 { "" } else { "s" }))
            .size(13.0)
            .color(colors::TEXT_PRIMARY),
    );
    let next = match tier {
        0 => 2,
        1 => 7,
        2 => 14,
        _ => 0,
    };
    if next > 0 && count < next {
        ui.label(
            egui::RichText::new(format!("{} more boosts for the next tier.", next - count))
                .size(12.0)
                .color(colors::TEXT_TERTIARY),
        );
    }
}

/// DM row context menu (pin/unpin). The actual toggle is handled by the
/// DM list renderer (it owns the config).
fn dm_context_ui(ui: &mut Ui, _app_state: &AppState, dm: Snowflake) {
    let resp = ui
        .add(egui::Button::new(
            egui::RichText::new("Pin / Unpin this conversation").size(13.5).color(colors::TEXT_PRIMARY),
        ))
        .on_hover_text("Pinned DMs stay at the top of the list");
    if resp.clicked() {
        ui.ctx()
            .data_mut(|d| d.insert_temp::<Snowflake>(egui::Id::new("pin_toggle_dm"), dm));
    }
}

fn render_dm_list(
    ui: &mut Ui,
    app_state: &AppState,
    rest: Arc<crate::rest::Http>,
    new_sel: &mut Option<Selection>,
    config: &mut crate::config::Config,
    body_bottom: f32,
) {
    // ── Home nav rows (Discord's sidebar home): Friends, Message
    // Requests, Nitro, Shop, Quests (point 22). Clicking shows the page
    // in the center panel via the HOME_PAGE selection marker.
    {
        let rows: [(&str, &str, &str); 5] = [
            ("person", "Friends", "friends"),
            ("person_add", "Message Requests", "requests"),
            ("diamond", "Nitro", "nitro"),
            ("storefront", "Shop", "shop"),
            ("sports_esports", "Quests", "quests"),
        ];
        for (icon, label, page) in rows {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
            let resp = ui
                .interact(rect, ui.id().with(("home_nav", page)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let active = ui
                .ctx()
                .data(|d| d.get_temp::<String>(egui::Id::new(crate::ui::home::HOME_PAGE)))
                .map(|p| p == page)
                .unwrap_or(false)
                && app_state.selection_sync().channel_id.is_none();
            let painter = ui.painter_at(rect);
            if active {
                painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.55));
            } else if resp.hovered() {
                painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30));
            }
            crate::icons::draw(&painter, icon, egui::pos2(rect.min.x + 26.0, rect.center().y), 20.0, colors::TEXT_SECONDARY);
            painter.text(
                egui::pos2(rect.min.x + 46.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(14.5),
                if active { colors::TEXT_PRIMARY } else { colors::TEXT_SECONDARY },
            );
            if resp.clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new(crate::ui::home::HOME_PAGE), page.to_string()));
                *new_sel = Some(Selection { guild_id: None, channel_id: None });
            }
        }
    }
    // Filter box.
    let filter_id = ui.id().with("dm_filter");
    let mut filter = ui.ctx().data(|d| d.get_temp::<String>(filter_id)).unwrap_or_default();
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let desired = ui.available_width() - 16.0;
        let resp = ui.add(
            egui::TextEdit::singleline(&mut filter)
                .desired_width(desired)
                .hint_text("Find a conversation")
                .text_color(colors::TEXT_PRIMARY),
        );
        if !resp.has_focus() && filter.is_empty() {
            // Idle look.
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(filter_id, filter.clone()));
    ui.add_space(4.0);

    let selected = app_state.selection_sync().channel_id;
    let mut dms = app_state.dm_channels();
    // Sort by the most recent message (fall back to id order).
    dms.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.map(|m| m.0).unwrap_or(0)));

    // Resolve DM recipients we only know by ID (private_channels_v2 form).
    for c in &dms {
        if c.recipients.is_empty() {
            if let Some(&rid) = c.recipient_ids.first() {
                resolve_user(rest.clone(), rid);
            }
        }
    }

    let needle = filter.to_lowercase();
    let filtered: Vec<Channel> = dms
        .into_iter()
        .filter(|c| {
            let name = c
                .dm_recipient(|id| app_state.user(id))
                .map(|u| u.display_name().to_lowercase())
                .unwrap_or_default();
            needle.is_empty() || name.contains(&needle)
        })
        .collect();

    let scroll_height = (body_bottom - ui.next_widget_position().y).max(48.0);

    // Pin toggles requested from the row context menu.
    let pin_toggle: Option<Snowflake> = ui
        .ctx()
        .data(|d| d.get_temp::<Snowflake>(egui::Id::new("pin_toggle_dm")));
    if let Some(dm) = pin_toggle {
        ui.ctx()
            .data_mut(|d| d.remove_temp::<Snowflake>(egui::Id::new("pin_toggle_dm")));
        let id_str = dm.0.to_string();
        if let Some(pos) = config.pinned_dms.iter().position(|x| *x == id_str) {
            config.pinned_dms.remove(pos);
            app_state.set_pinned_dm(dm, false);
            crate::ui::toast::info("Conversation unpinned.");
        } else {
            config.pinned_dms.push(id_str);
            app_state.set_pinned_dm(dm, true);
            crate::ui::toast::info("Conversation pinned to the top.");
        }
        let _ = config.save();
    }

    let pinned: Vec<&Channel> = filtered.iter().filter(|c| app_state.pinned_dm(c.id)).collect();
    let rest_list: Vec<&Channel> = filtered.iter().filter(|c| !app_state.pinned_dm(c.id)).collect();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(scroll_height)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if filtered.is_empty() {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new(if needle.is_empty() {
                            "No conversations yet."
                        } else {
                            "Nothing found."
                        })
                        .size(13.0)
                        .color(colors::TEXT_TERTIARY),
                    );
                });
            }
            if !pinned.is_empty() {
                section_label(ui, "PINNED");
                for c in &pinned {
                    render_dm_row(ui, app_state, c, selected == Some(c.id), new_sel);
                }
                ui.add_space(8.0);
            }
            if !rest_list.is_empty() {
                section_label(ui, "DIRECT MESSAGES");
            }
            for c in &rest_list {
                render_dm_row(ui, app_state, c, selected == Some(c.id), new_sel);
            }
            ui.add_space(12.0);
        });
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

fn render_dm_row(
    ui: &mut Ui,
    app_state: &AppState,
    c: &Channel,
    is_active: bool,
    new_sel: &mut Option<Selection>,
) {
    let row_h = 44.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
    let response = ui
        .interact(rect, ui.id().with(("dm", c.id.0)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect);

    let unread = app_state.unread_count(c.id);
    let mentions = app_state.mention_count(c.id);

    let hover_t = ui.ctx().animate_value_with_time(
        ui.id().with(("dm_hover", c.id.0)),
        if response.hovered() { 1.0 } else { 0.0 },
        0.09,
    );
    if is_active {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.55));
    } else if hover_t > 0.01 {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30 * hover_t));
    }

    let recipient = c.dm_recipient(|id| app_state.user(id));
    let name = recipient
        .as_ref()
        .map(|u| u.display_name().to_string())
        .unwrap_or_else(|| "Unknown".into());
    let avatar_url = recipient.as_ref().map(|u| u.avatar_url());
    let status = recipient
        .as_ref()
        .and_then(|u| app_state.presence(u.id))
        .unwrap_or_else(|| "offline".into());

    let avatar_rect = Rect::from_center_size(
        egui::pos2(rect.min.x + 32.0, rect.center().y),
        Vec2::splat(32.0),
    );
    crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
        match (&avatar_url, c.kind) {
            (Some(url), _) => {
                crate::image_loader::render_avatar(ui, url, 32.0, &name, Some(&status));
            }
            (None, ChannelType::GroupDm) => {
                crate::icons::draw(ui.painter(), "group", avatar_rect.center(), 20.0, colors::TEXT_TERTIARY);
            }
            (None, _) => {
                let p = ui.painter_at(avatar_rect);
                p.rect_filled(avatar_rect, 8.0, colors::BG_ACCENT);
                p.text(
                    avatar_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    name.chars().next().unwrap_or('?').to_uppercase().to_string(),
                    egui::FontId::proportional(14.0),
                    colors::TEXT_PRIMARY,
                );
            }
        }
    });

    let txt_color = if is_active || unread > 0 {
        colors::TEXT_PRIMARY
    } else {
        colors::TEXT_TERTIARY
    };
    let name_rect = Rect::from_min_max(
        egui::pos2(rect.min.x + 56.0, rect.min.y + 8.0),
        egui::pos2(rect.max.x - 12.0, rect.max.y - 8.0),
    );
    crate::ui::allocate_ui_at_rect(ui, name_rect, |ui| {
        ui.vertical(|ui| {
            emoji::render_label(ui, &name, 14.0, txt_color);
            if c.kind == ChannelType::GroupDm {
                let members = c.recipients.len().max(c.recipient_ids.len());
                if members > 1 {
                    ui.label(
                        egui::RichText::new(format!("{} members", members))
                            .size(11.0)
                            .color(colors::TEXT_MUTED),
                    );
                }
            }
        });
    });

    if mentions > 0 {
        let badge_r = if mentions > 9 { 9.5 } else { 8.0 };
        let center = egui::pos2(rect.max.x - 18.0, rect.center().y);
        painter.circle_filled(center, badge_r, colors::RED);
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            mentions.to_string(),
            egui::FontId::proportional(10.0),
            colors::TEXT_PRIMARY,
        );
    }

    // Unread dot at the far left.
    if !is_active && unread > 0 {
        painter.circle_filled(egui::pos2(rect.min.x + 7.0, rect.center().y), 3.0, colors::TEXT_PRIMARY);
    }

    // Selecting a DM clears the home-page marker.
    if response.clicked() {
        ui.ctx()
            .data_mut(|d| d.remove_temp::<String>(egui::Id::new(crate::ui::home::HOME_PAGE)));
        *new_sel = Some(Selection {
            guild_id: None,
            channel_id: Some(c.id),
        });
    }
    // Right-click: pin/unpin context popup.
    if response.secondary_clicked() {
        open_sidebar_popup(ui, c.id, SidePopup::DmContext { dm: c.id });
    }
}

// ───────────────────────────── user box ─────────────────────────────

fn render_user_box(ui: &mut Ui, app_state: &AppState) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 54.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors::BG_FLOATING);
    // Top divider.
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y),
            egui::pos2(rect.max.x, rect.min.y + 1.0),
        ),
        0.0,
        colors::BG_GUILDS_BAR,
    );

    let user = app_state.current_user();
    let status = app_state.own_status();

    // Profile banner as the box background (dimmed, Discord-style);
    // plain dark when there is none.
    if let Some(u) = &user {
        if let Some(banner) = u.banner_url() {
            crate::image_loader::draw_cover_image(ui, rect, &banner, 480, 96);
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, colors::BG_FLOATING.gamma_multiply(0.35));
        }
    }

    // Avatar (click = status picker).
    let avatar_rect = Rect::from_center_size(
        egui::pos2(rect.min.x + 26.0, rect.center().y),
        Vec2::splat(34.0),
    );
    let avatar_resp = ui
        .interact(avatar_rect, ui.id().with("user_box_avatar"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    crate::ui::allocate_ui_at_rect(ui, avatar_rect, |ui| {
        if let Some(u) = &user {
            crate::image_loader::render_avatar(ui, &u.avatar_url(), 34.0, u.display_name(), Some(&status));
        } else {
            let p = ui.painter_at(avatar_rect);
            p.rect_filled(avatar_rect, 10.0, colors::BG_ACCENT);
        }
    });

    // Name block (with the status dropdown chevron next to the name).
    let name_rect = Rect::from_min_max(
        egui::pos2(rect.min.x + 50.0, rect.min.y + 10.0),
        egui::pos2(rect.max.x - 104.0, rect.max.y - 10.0),
    );
    crate::ui::allocate_ui_at_rect(ui, name_rect, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                if let Some(u) = &user {
                    ui.label(
                        egui::RichText::new(u.display_name())
                            .color(colors::TEXT_PRIMARY)
                            .size(13.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!("@{}", u.username))
                            .color(colors::TEXT_TERTIARY)
                            .size(11.0),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("Not signed in")
                            .color(colors::TEXT_TERTIARY)
                            .size(13.0),
                    );
                }
            });
            // Status dropdown chevron (also opens the status popup).
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
            let resp = resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Set status");
            crate::icons::draw(ui.painter(), "expand_more", r.center(), 16.0, colors::TEXT_TERTIARY);
            let popup_id = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("status_open_flag")));
            let _ = popup_id;
            if resp.clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp::<bool>(egui::Id::new("open_status_now"), true));
            }
        });
    });

    // Right-side actions, symmetric: mic, headphones, gear (Discord's
    // exact order). Mic/deafen are session toggles that will apply to
    // voice when the WebRTC stack ships; the status dropdown opens with
    // the chevron (and the avatar, as before).
    {
        let popup_id = ui.id().with("status_popup");
        let status_open = ui.ctx().data(|d| d.get_temp::<bool>(popup_id)).unwrap_or(false);
        let mic_key = ui.id().with("user_box_mic");
        let deaf_key = ui.id().with("user_box_deaf");
        let mut mic_muted = ui.ctx().data(|d| d.get_temp::<bool>(mic_key)).unwrap_or(false);
        let mut deaf = ui.ctx().data(|d| d.get_temp::<bool>(deaf_key)).unwrap_or(false);
        for (idx, (key, icon, on_icon, tooltip, state)) in [
            (mic_key, "mic", "mic_off", "Mute microphone (applies when voice ships)", mic_muted),
            (deaf_key, "headphones", "volume_off", "Deafen audio (applies when voice ships)", deaf),
        ]
        .iter()
        .enumerate()
        {
            let r = Rect::from_center_size(
                egui::pos2(rect.max.x - 62.0 - idx as f32 * 30.0, rect.center().y),
                Vec2::splat(30.0),
            );
            let resp = ui
                .interact(r, ui.id().with(("user_box_act", idx)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(*tooltip);
            let active = *state;
            let c = if resp.hovered() || active {
                colors::RED
            } else {
                colors::TEXT_TERTIARY
            };
            crate::icons::draw(ui.painter(), if active { on_icon } else { icon }, r.center(), 20.0, c);
            if resp.clicked() {
                let new_state = !active;
                ui.ctx().data_mut(|d| d.insert_temp(*key, new_state));
                if *key == mic_key {
                    mic_muted = new_state;
                } else {
                    deaf = new_state;
                }
            }
        }
        let _ = (mic_muted, deaf);

        // Settings gear (far right).
        let srect = Rect::from_center_size(
            egui::pos2(rect.max.x - 26.0, rect.center().y),
            Vec2::splat(30.0),
        );
        let resp = ui
            .interact(srect, ui.id().with("user_box_settings"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("User settings");
        let c = if resp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY };
        crate::icons::draw(ui.painter(), "settings", srect.center(), 20.0, c);
        if resp.clicked() {
            app_state.request_settings_toggle();
        }
        let _ = status_open;
    }

    // ── Status picker popup ──
    let popup_id = ui.id().with("status_popup");
    let mut status_open = ui.ctx().data(|d| d.get_temp::<bool>(popup_id)).unwrap_or(false);
    let open_now = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(egui::Id::new("open_status_now")))
        .unwrap_or(false);
    if open_now {
        ui.ctx()
            .data_mut(|d| d.remove_temp::<bool>(egui::Id::new("open_status_now")));
        status_open = true;
    }
    if avatar_resp.clicked() {
        status_open = !status_open;
    }
    let mut set_status: Option<String> = None;
    if status_open {
        let mut open_mut = status_open;
        if let Some(_popup) = egui::Popup::from_response(&avatar_resp)
            .width(180.0)
            .frame(
                egui::Frame::new()
                    .fill(colors::BG_FLOATING)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(6))
                    .stroke(egui::Stroke::new(1.0, colors::BG_ACCENT)),
            )
            .open_bool(&mut open_mut)
            .show(|ui| {
                ui.set_min_width(170.0);
                for (key, label) in [
                    ("online", "Online"),
                    ("idle", "Idle"),
                    ("dnd", "Do not disturb"),
                    ("invisible", "Invisible"),
                ] {
                    let selected = status == key;
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.hovered() || selected {
                        ui.painter_at(rect).rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.4));
                    }
                    let painter = ui.painter_at(rect);
                    let dot_center = egui::pos2(rect.min.x + 16.0, rect.center().y);
                    let color = crate::colors::status_color(key);
                    painter.circle_filled(dot_center, 6.0, color);
                    if key == "invisible" {
                        painter.circle_filled(dot_center, 3.5, colors::BG_FLOATING);
                    }
                    painter.text(
                        egui::pos2(rect.min.x + 32.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(14.0),
                        colors::TEXT_PRIMARY,
                    );
                    if resp.clicked() {
                        set_status = Some(key.to_string());
                    }
                }
            })
        {}
        status_open = open_mut;
        if let Some(st) = set_status.take() {
            status_open = false;
            app_state.request_presence(&st);
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(popup_id, status_open));
}

// ───────────────────────────── helpers ─────────────────────────────

/// When a guild is selected without a channel, pick its first text channel
/// (Discord's behavior on server switch).
pub fn auto_select_first_channel(app_state: &AppState) {
    let sel = app_state.selection_sync();
    let Some(guild_id) = sel.guild_id else { return };
    if sel.channel_id.is_some() {
        return;
    }
    let mut channels = app_state.channels_for_guild(guild_id);
    channels.sort_by_key(|c| (c.parent_id.is_some(), c.position.unwrap_or(0)));
    if let Some(first) = channels.iter().find(|c| c.is_text_like()) {
        app_state.set_selection_sync(Selection {
            guild_id: Some(guild_id),
            channel_id: Some(first.id),
        });
    }
}

/// Fetch message history for a channel (deduped against in-flight fetches).
pub fn fetch_channel_messages(rest: &Arc<crate::rest::Http>, shared: &Arc<AppState>, channel_id: Snowflake) {
    if !INFLIGHT_FETCH.insert(channel_id.0) {
        return; // already fetching
    }
    let rest = rest.clone();
    let shared = shared.clone();
    let cid = channel_id;
    tokio::spawn(async move {
        let result = rest.get_channel_messages(cid, 50, None).await;
        INFLIGHT_FETCH.remove(&cid.0);
        match result {
            Ok(msgs) => {
                shared.set_messages(cid, msgs);
            }
            Err(e) => {
                tracing::warn!(error = %e, "fetch messages for channel {cid}");
            }
        }
    });
}

/// Fetch a user record by ID (once) so DM rows can render name + avatar.
fn resolve_user(rest: Arc<crate::rest::Http>, user_id: Snowflake) {
    if !INFLIGHT_FETCH.insert(user_id.0 ^ 0x_feed_feed) {
        return;
    }
    tokio::spawn(async move {
        let result = rest.get_user(user_id).await;
        INFLIGHT_FETCH.remove(&(user_id.0 ^ 0x_feed_feed));
        match result {
            Ok(u) => {
                if let Some(s) = state::global() {
                    s.touch_user(&u);
                }
            }
            Err(_) => {
                // Resolution failed; the placeholder avatar stays until
                // the next presence or member event for this user.
            }
        }
    });
}
