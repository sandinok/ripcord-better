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
    config: &crate::config::Config,
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
                render_channel_tree(ui, app_state, gid, &mut new_sel, body_bottom);
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
            render_channel_section(ui, app_state, &no_category, selected, new_sel, false);

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
                render_category_header(ui, app_state, cat, &children, selected);
                render_channel_section(ui, app_state, &children, selected, new_sel, true);
                ui.add_space(6.0);
            }
            ui.add_space(12.0);
        });
}

fn render_channel_section(
    ui: &mut Ui,
    app_state: &AppState,
    channels: &[&Channel],
    selected: Option<Snowflake>,
    new_sel: &mut Option<Selection>,
    indent: bool,
) {
    let mut sorted: Vec<&&Channel> = channels.iter().collect();
    sorted.sort_by_key(|c| c.position.unwrap_or(0));
    for c in sorted {
        render_channel_row(ui, app_state, c, selected == Some(c.id), new_sel, indent);
    }
}

fn render_category_header(
    ui: &mut Ui,
    app_state: &AppState,
    cat: &Channel,
    children: &[&Channel],
    _selected: Option<Snowflake>,
) {
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
    let _ = app_state;
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

    if is_active {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.55));
    } else if response.hovered() {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30));
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

    if response.clicked() {
        *new_sel = Some(Selection {
            guild_id: c.guild_id,
            channel_id: Some(c.id),
        });
    }
}

// ───────────────────────────── DM list ─────────────────────────────

fn render_dm_list(
    ui: &mut Ui,
    app_state: &AppState,
    rest: Arc<crate::rest::Http>,
    new_sel: &mut Option<Selection>,
    config: &crate::config::Config,
    body_bottom: f32,
) {
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
            for c in &filtered {
                render_dm_row(ui, app_state, c, selected == Some(c.id), new_sel);
            }
            // Friends shortcut (opens the DM channel too when clicked:
            // selects the first DM as a stand-in? No - it's a header only).
            let _ = config;
            ui.add_space(12.0);
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

    if is_active {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.55));
    } else if response.hovered() {
        painter.rect_filled(rect, 4.0, colors::BG_ACCENT.gamma_multiply(0.30));
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
            if c.kind == ChannelType::GroupDm && c.recipients.len() > 1 {
                ui.label(
                    egui::RichText::new(format!("{} members", c.recipients.len()))
                        .size(11.0)
                        .color(colors::TEXT_MUTED),
                );
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

    if response.clicked() {
        *new_sel = Some(Selection {
            guild_id: None,
            channel_id: Some(c.id),
        });
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

    // Name block.
    let name_rect = Rect::from_min_max(
        egui::pos2(rect.min.x + 50.0, rect.min.y + 10.0),
        egui::pos2(rect.max.x - 96.0, rect.max.y - 10.0),
    );
    crate::ui::allocate_ui_at_rect(ui, name_rect, |ui| {
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
    });

    // Settings action (right).
    {
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
    }

    // ── Status picker popup ──
    let popup_id = ui.id().with("status_popup");
    let mut status_open = ui.ctx().data(|d| d.get_temp::<bool>(popup_id)).unwrap_or(false);
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
