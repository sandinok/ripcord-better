//! Chat panel - the central column. Message history with dynamic-height
//! rows (nothing gets clipped), a header with the channel name + emoji,
//! hover actions, replies, reactions with color emoji, a typing indicator,
//! and a composer that is always visible with Enter-to-send.

use std::collections::HashSet;
use std::sync::Arc;

use egui::{Key, Rect, Sense, Ui, Vec2};

use crate::colors;
use crate::image_loader::render_avatar;
use crate::markdown::{self, MentionLookup};
use crate::model::{Channel, Message, Snowflake};
use crate::rest::endpoints::{AllowedMentions, CreateMessageBody};
use crate::state::AppState;
use crate::ui::emoji;

/// Resolves `<@id>` / `<@&id>` / `<#id>` mentions against the live app
/// state: user names from the user cache, role names + colors from the
/// current guild, channel names from the channel cache. With NoLookup the
/// renderer fell back to `@role-853...`-style raw ids.
struct StateLookup<'a> {
    state: &'a AppState,
    guild_id: Option<Snowflake>,
}

impl MentionLookup for StateLookup<'_> {
    fn user_label(&self, id: Snowflake) -> String {
        self.state
            .user(id)
            .map(|u| format!("@{}", u.display_name()))
            .unwrap_or_else(|| "@unknown-user".to_string())
    }
    fn role_label(&self, id: Snowflake) -> (String, egui::Color32) {
        if let Some(g) = self.guild_id.and_then(|g| self.state.guild_by_id(g)) {
            if let Some(r) = g.roles.iter().find(|r| r.id == id) {
                let color = if r.color == 0 {
                    colors::TEXT_SECONDARY
                } else {
                    crate::ui::members::role_color(r.color)
                };
                return (format!("@{}", r.name), color);
            }
        }
        ("@deleted-role".to_string(), colors::TEXT_SECONDARY)
    }
    fn channel_label(&self, id: Snowflake) -> String {
        self.state
            .channel_by_id(id)
            .map(|c| format!("#{}", c.display_name()))
            .unwrap_or_else(|| "#deleted-channel".to_string())
    }
}

pub struct ChatState {
    pub input: String,
    pub reply_to: Option<Snowflake>,
    pub spoilers_revealed: HashSet<usize>,
    pub auto_scroll: bool,
    /// Focus the composer as soon as a channel is selected (Discord-like).
    pub want_composer_focus: bool,
    /// Message the emoji picker is currently open for (hover "add reaction").
    pub reaction_picker_for: Option<Snowflake>,
    /// Live filter text of the emoji picker.
    pub reaction_search: String,
    /// When the emoji picker was opened; outside-clicks are ignored for a
    /// short grace window so the opening click cannot close it instantly.
    pub reaction_picker_opened: Option<std::time::Instant>,
    /// Last send error, shown inline above the composer until the user
    /// starts typing again. `None` normally; sends never auto-retry.
    pub send_error: Option<String>,
    /// Top-bar popup that is open: search box, pinned messages, or the
    /// unread inbox (Discord's three right-side header actions).
    pub header_popup: HeaderPopup,
    /// When the header popup was opened (grace window for the opening click).
    pub header_popup_opened: Option<std::time::Instant>,
    /// Live query of the search popup.
    pub search_query: String,
    /// When the current user card was opened (click on avatar/name).
    pub card_opened: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderPopup {
    None,
    Search,
    Pins,
    Inbox,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            input: String::new(),
            reply_to: None,
            spoilers_revealed: HashSet::new(),
            auto_scroll: true,
            want_composer_focus: true,
            reaction_picker_for: None,
            reaction_search: String::new(),
            reaction_picker_opened: None,
            send_error: None,
            header_popup: HeaderPopup::None,
            header_popup_opened: None,
            search_query: String::new(),
            card_opened: None,
        }
    }
}

pub fn render(
    ui: &mut Ui,
    app_state: &AppState,
    rest: Arc<crate::rest::Http>,
    sender: &tokio::sync::mpsc::UnboundedSender<crate::sender::SendRequest>,
    chat_state: &mut ChatState,
    config: &mut crate::config::Config,
) {
    let sel = app_state.selection_sync();
    let channel = sel.channel_id.and_then(|id| app_state.channel_by_id(id));

    ui.set_min_width(ui.available_width());
    ui.set_min_height(ui.available_height());

    let Some(ch) = channel else {
        // No channel: header + centered hint + disabled composer.
        egui::Panel::top("chat_header")
            .exact_size(48.0)
            .frame(egui::Frame::new().fill(colors::BG_CHAT).inner_margin(egui::Margin::same(0)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                render_header(ui, None, config, chat_state);
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors::BG_CHAT).inner_margin(egui::Margin::same(0)))
            .show(ui, |ui| {
                render_no_channel(ui);
            });
        egui::Panel::bottom("chat_composer")
            .exact_size(72.0)
            .frame(egui::Frame::new().fill(colors::BG_CHAT).inner_margin(egui::Margin::same(0)))
            .show_separator_line(false)
            .show(ui, |ui| {
                render_composer(ui, app_state, sender, chat_state, None);
            });
        return;
    };

    // ── Header (top panel) ──
    egui::Panel::top("chat_header")
        .exact_size(48.0)
        .frame(egui::Frame::new().fill(colors::BG_CHAT).inner_margin(egui::Margin::same(0)))
        .show_separator_line(false)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            render_header(ui, Some(&ch), config, chat_state);
        });

    // ── Composer (bottom panel, ALWAYS visible) ──
    // The panel height tracks its ACTUAL content: typing line, reply bar,
    // inline error and the input card. The old fixed 72px overflowed when
    // reply + typing stacked up, spilling gray placeholder rows below the
    // composer (the "ghost boxes" row).
    let typing_lines = app_state.typing_in(ch.id).len();
    let composer_h = 44.0 // input card
        + 10.0 // bottom breathing room
        + 8.0 // top gap
        + if typing_lines > 0 { 18.0 } else { 0.0 }
        + if chat_state.reply_to.is_some() { 22.0 } else { 0.0 }
        + if chat_state.send_error.is_some() { 18.0 } else { 0.0 };
    egui::Panel::bottom("chat_composer")
        .exact_size(composer_h)
        .frame(egui::Frame::new().fill(colors::BG_CHAT).inner_margin(egui::Margin::same(0)))
        .show_separator_line(false)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            render_composer(ui, app_state, sender, chat_state, Some(&ch));
        });

    // ── Message history (fills everything between) ──
    let messages = app_state.messages_for(ch.id);
    let lookup = StateLookup {
        state: app_state,
        guild_id: ch.guild_id,
    };
    let scroll_output = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(chat_state.auto_scroll)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let mut last_author_id: Option<Snowflake> = None;
            let mut last_day: Option<time::OffsetDateTime> = None;
            let mut last_author_ts: Option<u64> = None;
            for msg in messages.iter() {
                let dt = msg.timestamp_dt();
                let ms_epoch = dt
                    .map(|d| (d.unix_timestamp_nanos() / 1_000_000) as u64)
                    .unwrap_or(0);
                // Date divider between messages from different days, like
                // the official client ("September 3, 2026" / "Today").
                let day = dt.map(|d| replace_time(d, 0, 0, 0));
                if day.is_some() && day != last_day {
                    if let Some(d) = day {
                        render_date_divider(ui, d);
                    }
                    // A divider breaks author grouping.
                    last_author_id = None;
                    last_author_ts = None;
                }
                last_day = day;
                let grouped = last_author_id == Some(msg.author.id)
                    && last_author_ts
                        .map(|t| ms_epoch.saturating_sub(t) < 5 * 60 * 1000)
                        .unwrap_or(false);
                let compact = config.density == "compact";
                render_message_row(ui, app_state, msg, grouped, rest.clone(), chat_state, config.font_size, compact, &lookup);
                last_author_id = Some(msg.author.id);
                last_author_ts = Some(ms_epoch);
            }
            if messages.is_empty() {
                if app_state.is_fetched(ch.id) {
                    render_welcome(ui, &ch);
                } else {
                    render_loading(ui);
                }
            }
            ui.add_space(16.0);
        });
    // If the user scrolled away from the bottom, disable auto-scroll.
    let offset = scroll_output.state.offset.y;
    let inner = scroll_output.content_size.y;
    let outer = scroll_output.inner_rect.height();
    chat_state.auto_scroll = !(outer < inner && offset < inner - outer - 4.0);

    // ── Reaction picker popup (one per frame, anchored at its message) ──
    if let Some(pid) = chat_state.reaction_picker_for {
        let anchor = ui
            .ctx()
            .data(|d| d.get_temp::<egui::Pos2>(egui::Id::new("picker_anchor").with(pid.0)));
        if let Some(anchor) = anchor {
            if let Some(emo) = crate::ui::reaction_picker::show(
                ui,
                &mut chat_state.reaction_picker_for,
                &mut chat_state.reaction_search,
                chat_state.reaction_picker_opened,
                pid,
                anchor,
            ) {
                // Send the reaction and close the picker.
                chat_state.reaction_picker_for = None;
                chat_state.reaction_search.clear();
                chat_state.reaction_picker_opened = None;
                let rest_clone = rest.clone();
                let cid_v = ch.id;
                tokio::spawn(async move {
                    if let Err(e) = rest_clone.create_reaction(cid_v, pid, &emo).await {
                        tracing::warn!(error = %e, "add reaction");
                    }
                });
            }
        }
    }

    // ── User card (opened from a member row or a message author) ──
    if ui
        .ctx()
        .data(|d| d.get_temp::<crate::model::Snowflake>(egui::Id::new(crate::ui::members::CARD_USER_ID)))
        .is_some()
    {
        crate::ui::members::render_user_card(ui, app_state, chat_state.card_opened);
    } else {
        chat_state.card_opened = None;
    }

    // ── Header popups (search / pins / inbox) ──
    render_header_popup(ui, app_state, chat_state, &ch);
}

/// Zero out the time-of-day so two timestamps on the same day compare equal.
fn replace_time(d: time::OffsetDateTime, h: u8, m: u8, s: u8) -> time::OffsetDateTime {
    d.replace_time(time::Time::from_hms(h, m, s).unwrap_or(time::Time::MIDNIGHT))
}

/// "Today" / "Yesterday" / "September 3, 2026" separator, Discord-style:
/// a hairline with the date chip centered on it.
fn render_date_divider(ui: &mut Ui, day: time::OffsetDateTime) {
    let today = replace_time(time::OffsetDateTime::now_utc(), 0, 0, 0);
    let yesterday = today - time::Duration::days(1);
    let label = if day == today {
        "Today".to_string()
    } else if day == yesterday {
        "Yesterday".to_string()
    } else {
        day.format(
            &time::format_description::parse("[month repr:long] [day], [year]").unwrap_or_default(),
        )
        .unwrap_or_default()
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let y = rect.center().y;
    let inset = 8.0;
    painter.rect_filled(
        Rect::from_min_max(egui::pos2(rect.min.x + inset, y - 0.5), egui::pos2(rect.max.x - inset, y + 0.5)),
        0.0,
        colors::BG_ACCENT,
    );
    let galley = painter.layout(
        label,
        egui::FontId::proportional(12.0),
        colors::TEXT_MUTED,
        f32::INFINITY,
    );
    let chip = Rect::from_center_size(egui::pos2(rect.center().x, y), egui::vec2(galley.size().x + 16.0, 20.0));
    painter.rect_filled(chip, 3.0, colors::BG_CHAT);
    painter.galley(
        egui::pos2(chip.center().x - galley.size().x / 2.0, chip.center().y - galley.size().y / 2.0),
        galley,
        egui::Color32::WHITE,
    );
    ui.add_space(4.0);
}

// ───────────────────────────── header ─────────────────────────────

fn render_header(
    ui: &mut Ui,
    channel: Option<&Channel>,
    config: &mut crate::config::Config,
    chat_state: &mut ChatState,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors::BG_CHAT);

    let (icon, name, topic) = match channel {
        None => ("tag", "No channel".to_string(), None),
        Some(c) => {
            let icon = match c.kind {
                crate::model::ChannelType::Voice | crate::model::ChannelType::StageVoice => "volume_up",
                crate::model::ChannelType::Dm | crate::model::ChannelType::GroupDm => "chat_bubble",
                _ => "tag",
            };
            let name = c.display_name();
            (icon, name, c.topic.clone())
        }
    };

    // Channel icon.
    crate::icons::draw(
        &painter,
        icon,
        egui::pos2(rect.min.x + 26.0, rect.center().y),
        22.0,
        colors::TEXT_TERTIARY,
    );

    // Name (emoji-aware, bold).
    let name_rect = Rect::from_min_max(
        egui::pos2(rect.min.x + 46.0, rect.min.y + 10.0),
        egui::pos2(rect.max.x - 160.0, rect.max.y - 10.0),
    );
    crate::ui::allocate_ui_at_rect(ui, name_rect, |ui| {
        ui.horizontal(|ui| {
            emoji::render_label(ui, &name, 16.0, colors::TEXT_HEADER);
            if let Some(t) = topic.as_deref().filter(|t| !t.is_empty()) {
                ui.add_space(8.0);
                let short: String = t.chars().take(48).collect();
                let suffix = if t.chars().count() > 48 { "..." } else { "" };
                ui.label(
                    egui::RichText::new(format!("{short}{suffix}"))
                        .size(13.0)
                        .color(colors::TEXT_TERTIARY),
                );
            }
        });
    });

    // Divider under the header.
    ui.painter().rect_filled(
        Rect::from_min_max(
            egui::pos2(rect.min.x, rect.max.y),
            egui::pos2(rect.max.x, rect.max.y + 1.0),
        ),
        0.0,
        colors::BG_ACCENT.gamma_multiply(0.5),
    );

    // Right-side actions, official order: pins, inbox (bell), search,
    // members toggle. Each opens its popup below the header.
    let show_members_action = channel.map(|c| c.guild_id.is_some()).unwrap_or(false);
    let mut x = rect.max.x - 28.0;
    let mut popup_just_opened: Option<HeaderPopup> = None;
    if show_members_action {
        let m_rect = Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::splat(32.0));
        let resp = ui
            .interact(m_rect, ui.id().with("chat_toggle_members"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if config.show_members { "Hide member list" } else { "Show member list" });
        let c = if config.show_members || resp.hovered() {
            colors::TEXT_PRIMARY
        } else {
            colors::TEXT_TERTIARY
        };
        crate::icons::draw(ui.painter(), "group", m_rect.center(), 20.0, c);
        if resp.clicked() {
            config.show_members = !config.show_members;
            let _ = config.save();
        }
        x -= 36.0;
    }
    // Search.
    {
        let r = Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::splat(32.0));
        let resp = ui
            .interact(r, ui.id().with("chat_search"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Search messages");
        let active = chat_state.header_popup == HeaderPopup::Search;
        let c = if active || resp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY };
        crate::icons::draw(ui.painter(), "search", r.center(), 20.0, c);
        if resp.clicked() {
            chat_state.header_popup = if active { HeaderPopup::None } else { HeaderPopup::Search };
            if chat_state.header_popup == HeaderPopup::Search {
                popup_just_opened = Some(HeaderPopup::Search);
            }
            chat_state.search_query.clear();
        }
        x -= 36.0;
    }
    // Inbox (unread mentions).
    {
        let r = Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::splat(32.0));
        let resp = ui
            .interact(r, ui.id().with("chat_inbox"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Inbox (unreads)");
        let active = chat_state.header_popup == HeaderPopup::Inbox;
        let c = if active || resp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY };
        crate::icons::draw(ui.painter(), "notifications", r.center(), 20.0, c);
        if resp.clicked() {
            chat_state.header_popup = if active { HeaderPopup::None } else { HeaderPopup::Inbox };
            if chat_state.header_popup == HeaderPopup::Inbox {
                popup_just_opened = Some(HeaderPopup::Inbox);
            }
        }
        x -= 36.0;
    }
    // Pins.
    {
        let r = Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::splat(32.0));
        let resp = ui
            .interact(r, ui.id().with("chat_pins"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Pinned messages");
        let active = chat_state.header_popup == HeaderPopup::Pins;
        let c = if active || resp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY };
        crate::icons::draw(ui.painter(), "keep", r.center(), 20.0, c);
        if resp.clicked() {
            chat_state.header_popup = if active { HeaderPopup::None } else { HeaderPopup::Pins };
            if chat_state.header_popup == HeaderPopup::Pins {
                popup_just_opened = Some(HeaderPopup::Pins);
                // Kick a pins fetch (deduped in the popup itself).
                if let (Some(rest), Some(c)) = (crate::rest::global(), channel) {
                    fetch_pins(rest, c.id);
                }
            }
        }
    }
    if popup_just_opened.is_some() {
        chat_state.header_popup_opened = Some(std::time::Instant::now());
    }
}

// ───────────────────────────── empty states ─────────────────────────────

/// One unified message when no channel is selected. No contradictions:
/// the header shows "No channel", the body shows a single hint, and the
/// composer (in its own bottom panel) is visibly disabled.
fn render_no_channel(ui: &mut Ui) {
    ui.allocate_space(ui.available_size() / 2.0 - egui::vec2(0.0, 60.0));
    ui.vertical_centered(|ui| {
        ui.set_width(ui.available_width());
        crate::icons::draw(ui.painter(), "chat_bubble", ui.next_widget_position() + egui::vec2(0.0, 40.0), 56.0, colors::BG_ACCENT);
        ui.add_space(90.0);
        ui.label(
            egui::RichText::new("No channel selected")
                .size(20.0)
                .color(colors::TEXT_HEADER)
                .strong(),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Pick a channel from the sidebar, or open your DMs from Home.")
                .size(14.0)
                .color(colors::TEXT_TERTIARY),
        );
    });
    ui.allocate_space(ui.available_size());
}

fn render_welcome(ui: &mut Ui, ch: &Channel) {
    ui.add_space(48.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.add_space(24.0);
            ui.label(
                egui::RichText::new(format!("Welcome to #{}!", ch.name))
                    .size(22.0)
                    .color(colors::TEXT_HEADER)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("This is the start of the channel. Say hello!")
                    .size(14.0)
                    .color(colors::TEXT_TERTIARY),
            );
        });
    });
}

fn render_loading(ui: &mut Ui) {
    ui.add_space(60.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new("Loading messages...")
                .size(14.0)
                .color(colors::TEXT_TERTIARY),
        );
    });
}

// ───────────────────────────── message rows ─────────────────────────────

#[allow(clippy::too_many_arguments)]
fn render_message_row(
    ui: &mut Ui,
    app_state: &AppState,
    msg: &Message,
    grouped: bool,
    rest: Arc<crate::rest::Http>,
    chat_state: &mut ChatState,
    font_size: f32,
    compact: bool,
    lookup: &StateLookup<'_>,
) {
    let row_id = ui.id().with(("msg", msg.id.0));
    let hovered_prev = ui.ctx().data(|d| d.get_temp::<bool>(row_id)).unwrap_or(false);

    let frame = egui::Frame::new()
        .fill(if hovered_prev { colors::BG_MESSAGE_HOVER } else { egui::Color32::TRANSPARENT })
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: if grouped { 0 } else { if compact { 4 } else { 8 } },
            bottom: if grouped { 0 } else { if compact { 4 } else { 8 } },
        });

    let mut reply_clicked: Option<Snowflake> = None;

    let inner = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
            if grouped {
                // Timestamp column, visible only while hovered.
                let ts_rect = Rect::from_min_size(
                    ui.next_widget_position(),
                    egui::vec2(64.0, 20.0),
                );
                if hovered_prev {
                    if let Some(t) = msg.timestamp_dt() {
                        let time = t
                            .format(&time::format_description::parse("[hour]:[minute]").unwrap_or_default())
                            .unwrap_or_default();
                        ui.painter_at(ts_rect).text(
                            ts_rect.left_center(),
                            egui::Align2::LEFT_CENTER,
                            time,
                            egui::FontId::proportional(10.5),
                            colors::TEXT_MUTED,
                        );
                    }
                }
                ui.add_space(64.0);
            } else {
                // Avatar column: round 40px with initials fallback; a click
                // opens the user card.
                let url = msg.author.avatar_url();
                let name = msg.author.display_name().to_string();
                let presence = app_state.presence(msg.author.id);
                let avatar_rect = ui.next_widget_position();
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    render_avatar(ui, &url, 40.0, &name, presence.as_deref());
                });
                let av_rect = Rect::from_min_size(avatar_rect + egui::vec2(0.0, 2.0), Vec2::splat(40.0));
                let av_resp = ui
                    .interact(av_rect, ui.id().with(("msg_avatar", msg.id.0)), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if av_resp.clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(crate::ui::members::CARD_USER_ID), msg.author.id));
                    chat_state.card_opened = Some(std::time::Instant::now());
                }
                ui.add_space(12.0);
            }

            // Content column.
            ui.vertical(|ui| {
                ui.set_min_width(ui.available_width() - 84.0);
                let name_font = font_size.max(14.0);
                if !grouped {
                    ui.horizontal(|ui| {
                        let name_resp = ui.add(
                            egui::Label::new(
                                egui::RichText::new(msg.author.display_name())
                                    .color(author_color(app_state, msg, lookup))
                                    .size(name_font)
                                    .strong(),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("View profile");
                        if name_resp.clicked() {
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(crate::ui::members::CARD_USER_ID), msg.author.id));
                            chat_state.card_opened = Some(std::time::Instant::now());
                        }
                        if msg.author.bot {
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
                        if let Some(t) = msg.timestamp_dt() {
                            ui.label(
                                egui::RichText::new(
                                    t.format(
                                        &time::format_description::parse(
                                            "[month repr:short] [day], [hour]:[minute]",
                                        )
                                        .unwrap_or_default(),
                                    )
                                    .unwrap_or_default(),
                                )
                                .color(colors::TEXT_TERTIARY)
                                .size(11.0),
                            );
                        }
                    });
                    ui.add_space(2.0);
                }
                // Reply reference.
                if msg.referenced_message.as_ref().is_some() || msg.message_reference.is_some() {
                    ui.horizontal(|ui| {
                        let (line_rect, _) = ui.allocate_exact_size(egui::vec2(2.0, 14.0), Sense::hover());
                        ui.painter_at(line_rect).rect_filled(
                            Rect::from_min_size(
                                line_rect.min,
                                egui::vec2(2.0, 14.0),
                            ),
                            1.0,
                            colors::TEXT_MUTED,
                        );
                        ui.add_space(6.0);
                        if let Some(ref_msg) = msg.referenced_message.as_ref() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Replying to {}",
                                    ref_msg.author.display_name()
                                ))
                                .color(colors::TEXT_LINK)
                                .size(12.5),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("Replying to a message")
                                    .color(colors::TEXT_LINK)
                                    .size(12.5),
                            );
                        }
                    });
                    ui.add_space(2.0);
                }
                markdown::render_message_content(
                    ui,
                    &msg.content,
                    lookup,
                    font_size,
                    msg.id.0 as usize,
                    &mut chat_state.spoilers_revealed,
                );
                if msg.edited_timestamp.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
                    ui.label(
                        egui::RichText::new("(edited)")
                            .color(colors::TEXT_TERTIARY)
                            .size(10.5),
                    );
                }
                if !msg.attachments.is_empty() {
                    render_attachments(ui, msg);
                }
                if !msg.embeds.is_empty() {
                    render_embeds(ui, msg, font_size, lookup);
                }
                if !msg.reactions.is_empty() {
                    render_reactions(ui, msg, rest.clone());
                }
            });

            // Hover actions (reply + add reaction), right-aligned.
            if hovered_prev {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    let (r1, resp1) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                    let resp1 = resp1.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Reply");
                    crate::icons::draw(ui.painter(), "reply", r1.center(), 18.0, colors::TEXT_TERTIARY);
                    if resp1.clicked() {
                        reply_clicked = Some(msg.id);
                    }
                    let (r2, resp2) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                    let resp2 = resp2
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Add reaction");
                    crate::icons::draw(ui.painter(), "mood", r2.center(), 18.0, colors::TEXT_TERTIARY);
                    if resp2.clicked() {
                        // Toggle the emoji picker for this message.
                        if chat_state.reaction_picker_for == Some(msg.id) {
                            chat_state.reaction_picker_for = None;
                            chat_state.reaction_picker_opened = None;
                        } else {
                            chat_state.reaction_picker_for = Some(msg.id);
                            chat_state.reaction_picker_opened =
                                Some(std::time::Instant::now());
                        }
                    }
                    if chat_state.reaction_picker_for == Some(msg.id) {
                        // Publish the anchor so chat::render can position the
                        // popup after the loop (it renders once per frame).
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new("picker_anchor").with(msg.id.0),
                                r2.right_center(),
                            );
                        });
                    }
                });
            }
        });
    });


    // Store hover state for the next frame (one-frame lag, imperceptible).
    let hovered = inner.response.hovered() || ui.rect_contains_pointer(inner.response.rect);
    ui.ctx().data_mut(|d| d.insert_temp(row_id, hovered));

    if let Some(target) = reply_clicked {
        chat_state.reply_to = Some(target);
        chat_state.want_composer_focus = true;
    }
}

fn render_attachments(ui: &mut Ui, msg: &Message) {
    let images: Vec<_> = msg
        .attachments
        .iter()
        .filter(|a| a.content_type.as_deref().unwrap_or("").starts_with("image/"))
        .collect();
    if !images.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for a in images {
                let w = a.width.unwrap_or(200).min(300) as f32;
                crate::image_loader::render_image(
                    ui,
                    &a.url,
                    w,
                    crate::image_loader::Shape::Rounded(6),
                );
            }
        });
    }
    let other: Vec<_> = msg
        .attachments
        .iter()
        .filter(|a| !a.content_type.as_deref().unwrap_or("").starts_with("image/"))
        .collect();
    if !other.is_empty() {
        ui.add_space(4.0);
        for a in other {
            let frame = egui::Frame::new()
                .fill(colors::BG_SECONDARY_ALT)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 8));
            frame.show(ui, |ui| {
                ui.set_width(320.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    crate::icons::draw(ui.painter(), "attach_file", r.center(), 22.0, colors::TEXT_TERTIARY);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&a.filename)
                                .size(13.5)
                                .color(colors::TEXT_PRIMARY)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(humansize(a.size))
                                .size(11.0)
                                .color(colors::TEXT_TERTIARY),
                        );
                    });
                });
            });
        }
    }
}

/// Author line + title of an embed card (shared by both thumbnail layouts).
fn render_embed_header(ui: &mut Ui, e: &crate::model::Embed) {
    if let Some(author) = &e.author {
        ui.horizontal(|ui| {
            if let Some(icon) = &author.icon_url {
                crate::image_loader::render_image(
                    ui,
                    icon,
                    22.0,
                    crate::image_loader::Shape::Circle,
                );
            }
            ui.label(
                egui::RichText::new(&author.name)
                    .size(13.0)
                    .color(colors::TEXT_PRIMARY)
                    .strong(),
            );
        });
    }
    if let Some(title) = &e.title {
        if let Some(url) = &e.url {
            if ui
                .link(
                    egui::RichText::new(title)
                        .size(15.0)
                        .color(colors::TEXT_LINK)
                        .strong(),
                )
                .clicked()
            {
                // Open the source link in the system browser.
                let u = url.clone();
                let _ = open::that_detached(&u);
            }
        } else {
            ui.label(
                egui::RichText::new(title)
                    .size(15.0)
                    .color(colors::TEXT_PRIMARY)
                    .strong(),
            );
        }
    }
}

fn render_embeds(ui: &mut Ui, msg: &Message, font_size: f32, lookup: &StateLookup<'_>) {
    for e in &msg.embeds {
        // Discord's embed: a dark card with a single 4px color stripe on the
        // LEFT edge only (the old full-border stroke boxed the whole card
        // in blue).
        let stripe = crate::ui::members::role_color(e.color.unwrap_or(0x1F_8B_4C));
        let frame = egui::Frame::new()
            .fill(colors::EMBED_BG)
            .stroke(egui::Stroke::NONE)
            .inner_margin(egui::Margin { left: 16, right: 12, top: 8, bottom: 8 })
            .corner_radius(4.0);
        let resp = frame.show(ui, |ui| {
            ui.set_max_width(432.0);
            ui.vertical(|ui| {
                tracing::debug!(thumb = ?e.thumbnail.as_ref().map(|t| (t.url.clone(), t.width, t.height)), image = ?e.image.is_some(), "EMBED-DBG");
                // Discord layout: the thumbnail sits top-right beside the
                // author/title with its REAL aspect ratio (a forced 80x80
                // square used to stretch and gray-out unfurled thumbs);
                // the big image renders below the description.
                let has_thumb = e.thumbnail.is_some();
                if has_thumb {
                    let thumb = e.thumbnail.as_ref().unwrap();
                    let thumb_size = crate::image_loader::fit_size(
                        thumb.width,
                        thumb.height,
                        80.0,
                        80.0,
                    );
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width((432.0 - 28.0 - thumb_size.x).max(180.0));
                            render_embed_header(ui, e);
                            if let Some(desc) = &e.description {
                                ui.add_space(2.0);
                                let mut revealed = std::collections::HashSet::<usize>::new();
                                markdown::render_message_content(
                                    ui,
                                    desc,
                                    lookup,
                                    (font_size - 1.0).max(12.0),
                                    (msg.id.0 as usize) ^ 0xEBED,
                                    &mut revealed,
                                );
                            }
                        });
                        if let Some(url) = thumb.proxy_url.as_ref().or(Some(&thumb.url)) {
                            tracing::debug!(?url, ?thumb_size, cursor = ?ui.cursor(), avail = ui.available_width(), "THUMB-DBG");
                            crate::image_loader::render_image_size(
                                ui,
                                url,
                                thumb_size,
                                crate::image_loader::Shape::Rounded(4),
                            );
                        }
                    });
                } else {
                    render_embed_header(ui, e);
                    if let Some(desc) = &e.description {
                        ui.add_space(2.0);
                        let mut revealed = std::collections::HashSet::<usize>::new();
                        markdown::render_message_content(
                            ui,
                            desc,
                            lookup,
                            (font_size - 1.0).max(12.0),
                            (msg.id.0 as usize) ^ 0xEBED,
                            &mut revealed,
                        );
                    }
                }
                for f in &e.fields {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&f.name)
                            .size(13.0)
                            .color(colors::TEXT_PRIMARY)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(&f.value)
                            .size(13.0)
                            .color(colors::TEXT_SECONDARY),
                    );
                }
                if let Some(footer) = &e.footer {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&footer.text)
                            .size(12.0)
                            .color(colors::TEXT_TERTIARY),
                    );
                }
                // Big embed media (link unfurls, GIF posts): the image
                // Discord shows large under the description.
                if let Some(img) = e.image.as_ref() {
                    if let Some(url) = img.proxy_url.as_ref().or(Some(&img.url)) {
                        ui.add_space(4.0);
                        let size = crate::image_loader::fit_size(
                            img.width,
                            img.height,
                            384.0,
                            384.0,
                        );
                        crate::image_loader::render_image_size(
                            ui,
                            url,
                            size,
                            crate::image_loader::Shape::Rounded(6),
                        );
                    }
                }
            });
        });
        // The left stripe: painted onto the card's left edge after layout,
        // rounded on the left corners to hug the card.
        let cr = resp.response.rect;
        ui.painter_at(cr).rect_filled(
            Rect::from_min_max(egui::pos2(cr.min.x, cr.min.y), egui::pos2(cr.min.x + 4.0, cr.max.y)),
            4.0,
            stripe,
        );
        ui.add_space(4.0);
    }
}

fn render_reactions(ui: &mut Ui, msg: &Message, _rest: Arc<crate::rest::Http>) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for r in &msg.reactions {
            let frame = egui::Frame::new()
                .fill(if r.me { colors::MENTION_BG } else { colors::BG_ACCENT.gamma_multiply(0.6) })
                .stroke(if r.me {
                    egui::Stroke::new(1.0, colors::BLURPLE)
                } else {
                    egui::Stroke::new(1.0, egui::Color32::TRANSPARENT)
                })
                .inner_margin(egui::Margin::symmetric(8, 3))
                .corner_radius(8.0);
            frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Emoji in color: unicode via Twemoji, custom guild
                    // emoji straight from the Discord CDN (the old code
                    // printed ":simp:"-style text for those).
                    if let Some(url) = r.emoji.custom_emoji_url() {
                        crate::image_loader::render_emoji(
                            ui,
                            &url,
                            17.0,
                            r.emoji.name.as_deref().unwrap_or("emoji"),
                        );
                    } else if let Some(name) = r.emoji.name.as_deref() {
                        let url = emoji::twemoji_url(name);
                        let fallback = emoji::twemoji_url_vs16(name);
                        crate::image_loader::render_emoji_inline(ui, &url, &fallback, 17.0, name);
                    }
                    ui.label(
                        egui::RichText::new(r.count.to_string())
                            .size(12.5)
                            .color(colors::TEXT_PRIMARY),
                    );
                });
            });
        }
    });
}

// ───────────────────────────── composer ─────────────────────────────

fn render_composer(
    ui: &mut Ui,
    app_state: &AppState,
    sender: &tokio::sync::mpsc::UnboundedSender<crate::sender::SendRequest>,
    chat_state: &mut ChatState,
    channel: Option<&Channel>,
) {
    let cid = channel.map(|c| c.id);

    // Reply context bar.
    if let Some(target) = chat_state.reply_to {
        let messages = channel
            .map(|c| app_state.messages_for(c.id))
            .unwrap_or_default();
        if let Some(target_msg) = messages.iter().find(|m| m.id == target) {
            let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
            let painter = ui.painter_at(bar_rect);
            painter.rect_filled(
                Rect::from_min_size(egui::pos2(bar_rect.min.x + 40.0, bar_rect.max.y - 2.0), egui::vec2(40.0, 2.0)),
                1.0,
                colors::TEXT_MUTED,
            );
            painter.text(
                egui::pos2(bar_rect.min.x + 88.0, bar_rect.center().y),
                egui::Align2::LEFT_CENTER,
                format!("Replying to {}", target_msg.author.display_name()),
                egui::FontId::proportional(12.0),
                colors::TEXT_LINK,
            );
            // Cancel button.
            let x_rect = Rect::from_center_size(
                egui::pos2(bar_rect.min.x + 66.0, bar_rect.center().y),
                Vec2::splat(18.0),
            );
            let resp = ui
                .interact(x_rect, ui.id().with("cancel_reply"), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Cancel reply");
            crate::icons::draw(&painter, "close", x_rect.center(), 14.0, colors::TEXT_TERTIARY);
            if resp.clicked() {
                chat_state.reply_to = None;
            }
        }
    }

    // Typing indicator.
    if let Some(ch) = channel {
        let typers = app_state.typing_in(ch.id);
        if !typers.is_empty() {
            let (t_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
            let label = match typers.len() {
                1 => format!("{} is typing...", typers[0]),
                2 => format!("{} and {} are typing...", typers[0], typers[1]),
                _ => format!("{} and {} others are typing...", typers[0], typers.len() - 1),
            };
            ui.painter_at(t_rect).text(
                egui::pos2(t_rect.min.x + 24.0, t_rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(12.0),
                colors::TEXT_TERTIARY,
            );
        }
    }

    // Composer card.
    ui.add_space(4.0);

    // Restore a draft when a send failed: the text and error are parked in
    // the shared state by the failing task (it runs on a worker thread and
    // cannot touch the UI-owned ChatState directly).
    if let Some(cid_val) = cid {
        if let Some((err, draft, reply_to)) = app_state.take_failed_send(cid_val) {
        chat_state.input = draft;
        chat_state.reply_to = reply_to.or(chat_state.reply_to);
        chat_state.send_error = Some(err);
        chat_state.want_composer_focus = true;
        }
    }

    // Inline send-error line ("nothing was sent" is stated explicitly so
    // nobody re-sends blind and doubles a message).
    if let Some(err) = chat_state.send_error.clone() {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            crate::icons::draw(ui.painter(), "error", r.center(), 14.0, colors::RED);
            ui.label(
                egui::RichText::new(format!("{err} Your message was not sent."))
                    .size(12.0)
                    .color(colors::RED),
            );
        });
        ui.add_space(2.0);
    }

    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let card_w = ui.available_width() - 16.0;
        let frame = egui::Frame::new()
            .fill(colors::BG_INPUT)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(6, 5));
        frame.show(ui, |ui| {
            ui.set_width(card_w - 12.0);
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut chat_state.input)
                        .desired_width(ui.available_width() - 44.0)
                        .hint_text(match channel {
                            Some(c) => match c.kind {
                                crate::model::ChannelType::Dm | crate::model::ChannelType::GroupDm => {
                                    format!("Message {}", c.display_name())
                                }
                                _ => format!("Message #{}", c.name),
                            },
                            None => "Select a channel first".to_string(),
                        })
                        .text_color(colors::TEXT_PRIMARY),
                );
                if chat_state.want_composer_focus && channel.is_some() {
                    response.request_focus();
                    chat_state.want_composer_focus = false;
                }

                // Send button.
                let can_send = channel.is_some() && !chat_state.input.trim().is_empty();
                let (s_rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                let send_resp = ui
                    .interact(s_rect, ui.id().with("composer_send"), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Send");
                let sc = if can_send || send_resp.hovered() {
                    colors::BLURPLE
                } else {
                    colors::TEXT_MUTED
                };
                crate::icons::draw(ui.painter(), "send", s_rect.center(), 19.0, sc);
                if send_resp.clicked() && can_send {
                    send_current(sender, chat_state, cid);
                }

                // Enter-to-send, with exactly-once semantics.
                //
                // The singleline TextEdit reacts to Enter by surrendering
                // keyboard focus (in this same frame) but it does NOT consume
                // the key event, so any other Enter check in the same frame
                // can see the same press. `consume_key` removes the event
                // from the input state, which guarantees that one physical
                // Enter can trigger at most one send - ever. The composer
                // must own focus (or have just lost it to this very press) so
                // Enter presses aimed at other widgets (search box, modal)
                // never send messages.
                let composer_focused = response.has_focus() || response.lost_focus();
                if composer_focused {
                    let enter = ui.input_mut(|i| {
                        i.consume_key(egui::Modifiers::NONE, Key::Enter)
                    });
                    if enter {
                        chat_state.send_error = None;
                        if can_send {
                            send_current(sender, chat_state, cid);
                        }
                        // Discord keeps the composer focused after sending.
                        response.request_focus();
                    }
                }
                // Any edit to the input dismisses a stale send error.
                if response.changed() {
                    chat_state.send_error = None;
                }
            });
        });
    });
    ui.add_space(10.0);
}

fn send_current(
    sender: &tokio::sync::mpsc::UnboundedSender<crate::sender::SendRequest>,
    chat_state: &mut ChatState,
    cid: Option<Snowflake>,
) {
    let Some(cid) = cid else { return };
    let content = chat_state.input.trim().to_string();
    if content.is_empty() {
        return;
    }
    let reply_to = chat_state.reply_to.take();

    // Idempotency nonce: sent with the REST call and echoed back in the
    // MESSAGE_CREATE gateway event. Locally we insert an optimistic copy
    // keyed by the nonce; the first delivery (REST response or gateway
    // event) replaces it, so the user sees the message exactly once and
    // never double-sends. See state::AppState::insert_pending_message.
    let nonce = crate::state::new_nonce();
    let nonce_str = nonce.0.to_string();

    // Optimistic echo: the author's own message appears in the history
    // immediately, before the network round-trip.
    if let Some(s) = crate::state::global() {
        let author = s.current_user().unwrap_or_default();
        s.insert_pending_message(
            cid,
            &crate::model::Message {
                id: nonce,
                channel_id: cid,
                author,
                content: content.clone(),
                nonce: Some(nonce_str.clone()),
                message_reference: reply_to.map(|id| crate::model::MessageReference {
                    message_id: Some(id),
                    channel_id: Some(cid),
                    guild_id: None,
                }),
                ..Default::default()
            },
        );
    }

    let body = CreateMessageBody {
        content: Some(content),
        nonce: Some(nonce_str.clone()),
        tts: Some(false),
        embeds: Vec::new(),
        attachments: Vec::new(),
        message_reference: reply_to.map(|id| crate::model::MessageReference {
            message_id: Some(id),
            channel_id: Some(cid),
            guild_id: None,
        }),
        flags: None,
        allowed_mentions: Some(AllowedMentions {
            parse: vec!["users".into(), "roles".into()],
            ..Default::default()
        }),
    };
    chat_state.input.clear();
    chat_state.send_error = None;
    // Hand the message to the single send worker: one queue entry = one
    // REST POST, in order, never retried (see src/sender.rs).
    let _ = sender.send((cid, body, nonce_str));
}

fn humansize(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        return format!("{v} {}", UNITS[0]);
    }
    format!("{:.1} {}", v, UNITS[i])
}

// ───────────────────────────── header popups ─────────────────────────────

/// The author's name color: their highest-ranked colored role, like the
/// official client. Falls back to plain text when the guild or roles are
/// unknown (DMs).
fn author_color(app_state: &AppState, msg: &Message, lookup: &StateLookup<'_>) -> egui::Color32 {
    let Some(guild_id) = lookup.guild_id else {
        return colors::TEXT_PRIMARY;
    };
    let Some(guild) = app_state.guild_by_id(guild_id) else {
        return colors::TEXT_PRIMARY;
    };
    let member = guild.members.iter().find(|m| m.user.as_ref().map(|u| u.id) == Some(msg.author.id));
    let Some(role_ids) = member.map(|m| m.roles.clone()) else {
        return colors::TEXT_PRIMARY;
    };
    let mut best: Option<(i32, egui::Color32)> = None;
    for rid in &role_ids {
        if let Some(role) = guild.roles.iter().find(|r| r.id == *rid) {
            if role.color != 0 {
                let better = match best {
                    Some((pos, _)) => role.position > pos,
                    None => true,
                };
                if better {
                    best = Some((role.position, crate::ui::members::role_color(role.color)));
                }
            }
        }
    }
    best.map(|(_, c)| c).unwrap_or(colors::TEXT_PRIMARY)
}

/// Fetch (and cache) a channel's pins once per channel per session.
fn fetch_pins(rest: Arc<crate::rest::Http>, channel_id: Snowflake) {
    static INFLIGHT: once_cell::sync::Lazy<dashmap::DashSet<u64>> = once_cell::sync::Lazy::new(dashmap::DashSet::new);
    if !INFLIGHT.insert(channel_id.0) {
        return;
    }
    tokio::spawn(async move {
        match rest.get_channel_pins(channel_id).await {
            Ok(msgs) => {
                if let Some(s) = crate::state::global() {
                    s.set_pins(channel_id, msgs);
                    let _ = s.event_sender().send(crate::gateway::events::Event::RepaintRequested);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "fetch pins");
                // Allow a retry on the next open if it failed.
                INFLIGHT.remove(&channel_id.0);
            }
        }
    });
}

/// Render the currently-open header popup: message search (filters the
/// cached history live), pinned messages (REST), or the unread inbox.
fn render_header_popup(ui: &mut Ui, app_state: &AppState, chat_state: &mut ChatState, ch: &Channel) {
    if chat_state.header_popup == HeaderPopup::None {
        return;
    }
    let vp = ui.ctx().viewport_rect();
    let w = 340.0;
    let h = 260.0;
    // Below the header, right-aligned with the action icons.
    let pos = egui::pos2(
        (vp.max.x - w - 12.0).max(vp.min.x + 4.0),
        (vp.min.y + 52.0).max(vp.min.y + 52.0),
    );
    let area_rect = Rect::from_min_size(pos, egui::vec2(w, h));
    let frame = egui::Frame::new()
        .fill(colors::BG_FLOATING)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(10))
        .stroke(egui::Stroke::new(1.0, colors::BG_INPUT));
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    // Grace window: the click that opened the popup lands outside its rect
    // in this same frame and would otherwise close it instantly (the same
    // flash-open-flash-close class of bug the settings modal had).
    let in_grace = chat_state
        .header_popup_opened
        .map(|t| t.elapsed() < std::time::Duration::from_millis(250))
        .unwrap_or(true);
    egui::Area::new(egui::Id::new("header_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(area_rect.min)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(w - 20.0);
                ui.set_min_height(h - 20.0);
                ui.vertical(|ui| {
                    match chat_state.header_popup {
                        HeaderPopup::Search => {
                            ui.label(
                                egui::RichText::new("Search in this channel")
                                    .size(13.0)
                                    .strong()
                                    .color(colors::TEXT_PRIMARY),
                            );
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut chat_state.search_query)
                                    .desired_width(ui.available_width())
                                    .hint_text("Type to filter loaded history…")
                                    .font(egui::FontId::proportional(12.5)),
                            );
                            resp.request_focus();
                            let needle = chat_state.search_query.trim().to_lowercase();
                            let msgs = app_state.messages_for(ch.id);
                            let hits: Vec<&Message> = msgs
                                .iter()
                                .filter(|m| !needle.is_empty() && m.content.to_lowercase().contains(&needle))
                                .rev()
                                .take(20)
                                .collect();
                            ui.add_space(4.0);
                            if needle.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!("{} messages loaded in this channel", msgs.len()))
                                        .size(12.0)
                                        .color(colors::TEXT_TERTIARY),
                                );
                            } else if hits.is_empty() {
                                ui.label(
                                    egui::RichText::new("No matches.")
                                        .size(12.0)
                                        .color(colors::TEXT_TERTIARY),
                                );
                            }
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for m in hits {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(m.author.display_name())
                                                    .size(12.0)
                                                    .strong()
                                                    .color(colors::TEXT_PRIMARY),
                                            );
                                            let snippet: String = m
                                                .content
                                                .chars()
                                                .take(48)
                                                .collect();
                                            ui.label(
                                                egui::RichText::new(snippet)
                                                    .size(12.0)
                                                    .color(colors::TEXT_SECONDARY),
                                            );
                                        });
                                    }
                                });
                        }
                        HeaderPopup::Pins => {
                            ui.label(
                                egui::RichText::new("Pinned messages")
                                    .size(13.0)
                                    .strong()
                                    .color(colors::TEXT_PRIMARY),
                            );
                            ui.add_space(4.0);
                            let pins = app_state.pins_for(ch.id);
                            if pins.is_empty() {
                                ui.label(
                                    egui::RichText::new(if app_state.is_fetched(ch.id) {
                                        "Nothing pinned here yet."
                                    } else {
                                        "Loading pins…"
                                    })
                                    .size(12.0)
                                    .color(colors::TEXT_TERTIARY),
                                );
                            }
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for m in &pins {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(m.author.display_name())
                                                    .size(12.0)
                                                    .strong()
                                                    .color(colors::TEXT_PRIMARY),
                                            );
                                            let snippet: String = m
                                                .content
                                                .chars()
                                                .take(48)
                                                .collect();
                                            ui.label(
                                                egui::RichText::new(snippet)
                                                    .size(12.0)
                                                    .color(colors::TEXT_SECONDARY),
                                            );
                                        });
                                    }
                                });
                        }
                        HeaderPopup::Inbox => {
                            ui.label(
                                egui::RichText::new("Inbox — unread activity")
                                    .size(13.0)
                                    .strong()
                                    .color(colors::TEXT_PRIMARY),
                            );
                            ui.add_space(4.0);
                            let unreads: Vec<(Snowflake, u32, u32)> = app_state
                                .unread_channels()
                                .into_iter()
                                .collect();
                            if unreads.is_empty() {
                                ui.label(
                                    egui::RichText::new("You're all caught up.")
                                        .size(12.0)
                                        .color(colors::TEXT_TERTIARY),
                                );
                            }
                            for (cid, n, mentions) in unreads {
                                let label = app_state
                                    .channel_by_id(cid)
                                    .map(|c| c.display_name())
                                    .unwrap_or_else(|| "unknown channel".into());
                                let resp = ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "{} — {} unread{}",
                                                label,
                                                n,
                                                if mentions > 0 { format!(" ({mentions} mentions)") } else { String::new() }
                                            ))
                                            .size(12.5)
                                            .color(colors::TEXT_PRIMARY),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                if resp.clicked() {
                                    // Jump to the channel.
                                    app_state.set_selection_sync(crate::state::Selection {
                                        guild_id: ch.guild_id,
                                        channel_id: Some(cid),
                                    });
                                    if let Some(s) = crate::state::global() {
                                        s.mark_read(cid);
                                    }
                                    close = true;
                                }
                            }
                        }
                        HeaderPopup::None => {}
                    }
                });
            });
        });
    // Outside click closes (after the grace window).
    let outside = !in_grace
        && ui.input(|i| {
            i.pointer.button_clicked(egui::PointerButton::Primary)
                && i.pointer
                    .interact_pos()
                    .map(|p| !area_rect.contains(p))
                    .unwrap_or(false)
        });
    if close || outside {
        chat_state.header_popup = HeaderPopup::None;
        chat_state.header_popup_opened = None;
        chat_state.search_query.clear();
    }
}

// ───────────────────────────── security tests ─────────────────────────────
//
// The double-send bug: one Enter produced two REST POSTs. Root cause was
// the composer's Enter handling reading the same raw key event through
// two overlapping clauses (`lost_focus && key_pressed` OR
// `has_focus && key_pressed`) while immediately re-requesting focus, so
// the un-consumed event stayed visible to later checks. These tests pin
// the exactly-once guarantee against the REAL composer code.

#[cfg(test)]
mod send_tests {
    use super::*;
    use crate::model::{Channel, ChannelType, Snowflake};
    use crate::state::AppState;

    fn test_channel() -> Channel {
        Channel {
            id: Snowflake(42),
            kind: ChannelType::Text,
            name: "general".into(),
            ..Default::default()
        }
    }

    fn enter_event() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        app: &AppState,
        sender: &tokio::sync::mpsc::UnboundedSender<crate::sender::SendRequest>,
        chat: &mut ChatState,
    ) {
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            time: Some(1.0),
            ..Default::default()
        };
        let ch = test_channel();
        ctx.set_fonts(egui::FontDefinitions::empty());
        let output = ctx.run_ui(input, |ui| {
            ui.set_min_size(egui::vec2(900.0, 700.0));
            super::render_composer(ui, app, sender, chat, Some(&ch));
        });
        // Like egui's own __run_test_ui: discard textures without applying.
        output.drop_without_applying_deltas();
    }

    fn drain(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::sender::SendRequest>,
    ) -> Vec<crate::sender::SendRequest> {
        let mut out = Vec::new();
        while let Ok(r) = rx.try_recv() {
            out.push(r);
        }
        out
    }

    /// ONE Enter press with the composer focused must enqueue EXACTLY ONE
    /// send. Frames: focus first, then the Enter event, then two idle
    /// frames (a same-frame second check or a cross-frame re-read of the
    /// event would fire a second send - this is the regression test for
    /// the bug that duplicated messages).
    #[test]
    fn single_enter_sends_exactly_once() {
        let ctx = egui::Context::default();
        let app = AppState::new();
        let (tx, mut rx) = crate::sender::channel();
        let mut chat = ChatState {
            input: "hello world".into(),
            want_composer_focus: true,
            ..Default::default()
        };

        // Frame 1: focus the composer (want_composer_focus path).
        run_frame(&ctx, Vec::new(), &app, &tx, &mut chat);
        assert!(drain(&mut rx).is_empty(), "no send before Enter");

        // Frame 2: the single Enter press.
        run_frame(&ctx, vec![enter_event()], &app, &tx, &mut chat);
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 1, "one Enter must enqueue exactly one send");
        assert_eq!(sent[0].1.content.as_deref(), Some("hello world"));
        assert!(!sent[0].2.is_empty(), "send carries a nonce");
        assert!(chat.input.is_empty(), "input clears after send");

        // Frames 3 and 4: idle. The old bug could re-fire here because the
        // Enter event stayed visible while focus churned.
        run_frame(&ctx, Vec::new(), &app, &tx, &mut chat);
        run_frame(&ctx, Vec::new(), &app, &tx, &mut chat);
        assert!(drain(&mut rx).is_empty(), "no extra sends after idle frames");
    }

    /// Enter with focus somewhere else must NOT send (the DM search box
    /// must be able to receive Enter without the chat composer firing).
    #[test]
    fn enter_without_composer_focus_does_not_send() {
        let ctx = egui::Context::default();
        let app = AppState::new();
        let (tx, mut rx) = crate::sender::channel();
        let mut chat = ChatState {
            input: "hello world".into(),
            want_composer_focus: false,
            ..Default::default()
        };

        // Frame 1: no focus interaction.
        run_frame(&ctx, Vec::new(), &app, &tx, &mut chat);
        // Frame 2: Enter while the composer never held focus.
        run_frame(&ctx, vec![enter_event()], &app, &tx, &mut chat);
        assert!(
            drain(&mut rx).is_empty(),
            "Enter outside the composer must not send"
        );
        assert_eq!(chat.input, "hello world", "input is preserved");
    }

    /// Two separate Enter presses = two sends (fast consecutive messages
    /// are legitimate; only DUPLICATES of one press are the bug).
    #[test]
    fn two_enters_send_twice() {
        let ctx = egui::Context::default();
        let app = AppState::new();
        let (tx, mut rx) = crate::sender::channel();
        let mut chat = ChatState {
            input: "first".into(),
            want_composer_focus: true,
            ..Default::default()
        };
        run_frame(&ctx, Vec::new(), &app, &tx, &mut chat);
        run_frame(&ctx, vec![enter_event()], &app, &tx, &mut chat);
        chat.input = "second".into();
        run_frame(&ctx, vec![enter_event()], &app, &tx, &mut chat);
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 2, "two presses, two sends");
        assert_eq!(sent[0].1.content.as_deref(), Some("first"));
        assert_eq!(sent[1].1.content.as_deref(), Some("second"));
        // Nonces must differ (idempotency keys are per-message).
        assert_ne!(sent[0].2, sent[1].2);
    }

    /// Enter on an empty composer sends nothing and does not lose the draft.
    #[test]
    fn enter_with_empty_input_sends_nothing() {
        let ctx = egui::Context::default();
        let app = AppState::new();
        let (tx, mut rx) = crate::sender::channel();
        let mut chat = ChatState {
            input: "   ".into(), // whitespace-only trims to empty
            want_composer_focus: true,
            ..Default::default()
        };
        run_frame(&ctx, Vec::new(), &app, &tx, &mut chat);
        run_frame(&ctx, vec![enter_event()], &app, &tx, &mut chat);
        assert!(drain(&mut rx).is_empty());
    }
}
