//! Chat panel - the central column. Message history with dynamic-height
//! rows (nothing gets clipped), a header with the channel name + emoji,
//! hover actions, replies, reactions with color emoji, a typing indicator,
//! and a composer that is always visible with Enter-to-send.

use std::collections::HashSet;
use std::sync::Arc;

use egui::{Key, Rect, Sense, Ui, Vec2};

use crate::colors;
use crate::image_loader::render_avatar;
use crate::markdown::{self, NoLookup};
use crate::model::{Channel, Message, Snowflake};
use crate::rest::endpoints::{AllowedMentions, CreateMessageBody};
use crate::state::AppState;
use crate::ui::emoji;

pub struct ChatState {
    pub input: String,
    pub reply_to: Option<Snowflake>,
    pub spoilers_revealed: HashSet<usize>,
    pub auto_scroll: bool,
    /// Focus the composer as soon as a channel is selected (Discord-like).
    pub want_composer_focus: bool,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            input: String::new(),
            reply_to: None,
            spoilers_revealed: HashSet::new(),
            auto_scroll: true,
            want_composer_focus: true,
        }
    }
}

pub fn render(
    ui: &mut Ui,
    app_state: &AppState,
    rest: Arc<crate::rest::Http>,
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
                render_header(ui, None, config);
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
                render_composer(ui, app_state, rest, chat_state, None);
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
            render_header(ui, Some(&ch), config);
        });

    // ── Composer (bottom panel, ALWAYS visible) ──
    let typing_lines = app_state.typing_in(ch.id).len();
    let composer_h = 72.0 + if typing_lines > 0 { 20.0 } else { 0.0 } + if chat_state.reply_to.is_some() { 24.0 } else { 0.0 };
    egui::Panel::bottom("chat_composer")
        .exact_size(composer_h)
        .frame(egui::Frame::new().fill(colors::BG_CHAT).inner_margin(egui::Margin::same(0)))
        .show_separator_line(false)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            render_composer(ui, app_state, rest.clone(), chat_state, Some(&ch));
        });

    // ── Message history (fills everything between) ──
    let messages = app_state.messages_for(ch.id);
    let scroll_output = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(chat_state.auto_scroll)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let mut last_author_id: Option<Snowflake> = None;
            let mut last_author_ts: Option<u64> = None;
            for msg in messages.iter() {
                let ms_epoch = msg
                    .timestamp_dt()
                    .map(|d| (d.unix_timestamp_nanos() / 1_000_000) as u64)
                    .unwrap_or(0);
                let grouped = last_author_id == Some(msg.author.id)
                    && last_author_ts
                        .map(|t| ms_epoch.saturating_sub(t) < 5 * 60 * 1000)
                        .unwrap_or(false);
                let compact = config.density == "compact";
                render_message_row(ui, app_state, msg, grouped, rest.clone(), chat_state, config.font_size, compact);
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
}

// ───────────────────────────── header ─────────────────────────────

fn render_header(ui: &mut Ui, channel: Option<&Channel>, config: &mut crate::config::Config) {
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

    // Right-side action: toggle the member list (guild channels only).
    if channel.map(|c| c.guild_id.is_some()).unwrap_or(false) {
        let m_rect = Rect::from_center_size(
            egui::pos2(rect.max.x - 28.0, rect.center().y),
            Vec2::splat(32.0),
        );
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
    let mut quick_react: Option<(Snowflake, &str)> = None;

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
                // Avatar column.
                let url = msg.author.avatar_url();
                let name = msg.author.display_name().to_string();
                let presence = app_state.presence(msg.author.id);
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    render_avatar(ui, &url, 40.0, &name, presence.as_deref());
                });
                ui.add_space(12.0);
            }

            // Content column.
            ui.vertical(|ui| {
                ui.set_min_width(ui.available_width() - 84.0);
                let name_font = font_size.max(14.0);
                if !grouped {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(msg.author.display_name())
                                .color(colors::TEXT_PRIMARY)
                                .size(name_font)
                                .strong(),
                        );
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
                    &NoLookup,
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
                    render_embeds(ui, msg, font_size);
                }
                if !msg.reactions.is_empty() {
                    render_reactions(ui, msg, rest.clone());
                }
            });

            // Hover actions (reply + quick reaction), right-aligned.
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
                        quick_react = Some((msg.id, "🔥"));
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
    if let Some((mid, emo)) = quick_react {
        let rest_clone = rest.clone();
        let cid = msg.channel_id;
        tokio::spawn(async move {
            if let Err(e) = rest_clone.create_reaction(cid, mid, emo).await {
                tracing::warn!(error = %e, "add reaction");
            }
        });
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

fn render_embeds(ui: &mut Ui, msg: &Message, font_size: f32) {
    for e in &msg.embeds {
        let color = e.color.unwrap_or(0x1F_8B_4C) >> 8;
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;
        let stripe = egui::Color32::from_rgb(r, g, b);
        let frame = egui::Frame::new()
            .fill(colors::EMBED_BG)
            .stroke(egui::Stroke::new(4.0, stripe))
            .inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 8 })
            .corner_radius(4.0);
        frame.show(ui, |ui| {
            ui.set_max_width(432.0);
            ui.vertical(|ui| {
                if let Some(title) = &e.title {
                    ui.label(
                        egui::RichText::new(title)
                            .size(16.0)
                            .color(colors::TEXT_PRIMARY)
                            .strong(),
                    );
                }
                if let Some(desc) = &e.description {
                    ui.add_space(4.0);
                    let mut revealed = std::collections::HashSet::<usize>::new();
                    markdown::render_message_content(
                        ui,
                        desc,
                        &NoLookup,
                        (font_size - 1.0).max(12.0),
                        (msg.id.0 as usize) ^ 0xEBED,
                        &mut revealed,
                    );
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
            });
        });
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
                    // Emoji rendered in color (image) + count.
                    if r.emoji.id.is_none() {
                        if let Some(name) = r.emoji.name.as_deref() {
                            let url = emoji::twemoji_url(name);
                            let fallback = emoji::twemoji_url_vs16(name);
                            crate::image_loader::render_emoji_inline(ui, &url, &fallback, 17.0, name);
                        }
                    } else {
                        ui.label(
                            egui::RichText::new(format!(":{}:", r.emoji.name.clone().unwrap_or_default()))
                                .size(12.0)
                                .color(colors::TEXT_TERTIARY),
                        );
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
    rest: Arc<crate::rest::Http>,
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
                    send_current(ui, rest.clone(), chat_state, cid);
                }

                // Enter to send (singleline TextEdit: Enter = focus loss).
                let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let enter_down = ui.input(|i| i.key_pressed(Key::Enter));
                if (enter_pressed || (response.has_focus() && enter_down)) && can_send {
                    send_current(ui, rest, chat_state, cid);
                    response.request_focus();
                }
            });
        });
    });
    ui.add_space(10.0);
}

fn send_current(
    _ui: &mut Ui,
    rest: Arc<crate::rest::Http>,
    chat_state: &mut ChatState,
    cid: Option<Snowflake>,
) {
    let Some(cid) = cid else { return };
    let content = chat_state.input.trim().to_string();
    if content.is_empty() {
        return;
    }
    let reply_to = chat_state.reply_to.take();
    let body = CreateMessageBody {
        content: Some(content),
        nonce: None,
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
    tokio::spawn(async move {
        if let Err(e) = rest.post_message(cid, &body).await {
            tracing::warn!(error = %e, "send message");
        }
    });
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
