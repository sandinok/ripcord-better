//! The eframe::App implementation. Owns the AppState + REST Http client +
//! gateway handle + UI state (chat composer input, login form state,
//! settings modal, per-frame event pump).
//!
//! Layout: Discord-style panel columns, each a real `egui::Panel` so every
//! column stretches to the full window height (no black voids):
//!
//! ```text
//! | 72px guilds | 240px channels | chat (fills) | 240px members |
//! ```

use std::sync::Arc;

use eframe::egui;

use crate::colors;
use crate::config::Config;
use crate::gateway::{Gateway, Outbound};
use crate::rest::Http;
use crate::state::{self, AppState, Selection};
use crate::ui::{chat::ChatState, login::LoginState, settings::SettingsState};

pub struct AppInit {
    pub config: Config,
    pub runtime_handle: tokio::runtime::Handle,
}

pub struct BasaltApp {
    pub config: Config,
    pub shared: Arc<AppState>,
    pub rest: Arc<Http>,
    /// Single send queue: the composer pushes, one worker POSTs (see
    /// src/sender.rs). One Enter = exactly one message, ever.
    pub send_queue: tokio::sync::mpsc::UnboundedSender<crate::sender::SendRequest>,
    pub gateway_tx: tokio::sync::mpsc::UnboundedSender<Outbound>,
    pub _gateway_task: Option<Arc<Gateway>>,
    pub event_rx: tokio::sync::mpsc::UnboundedReceiver<crate::gateway::events::Event>,
    pub chat: ChatState,
    pub login: LoginState,
    pub settings: SettingsState,
    pub first_frame: bool,
    /// One-shot: jump to the first guild's first channel after login
    /// (Discord opens on your last destination; first run = first guild).
    pub auto_selected: bool,
    /// Cached `use_legacy_status_dots` value; if the user toggles the
    /// setting in the settings modal, we re-apply the theme next frame.
    pub last_legacy_dots: bool,
    pub runtime_handle: tokio::runtime::Handle,
}

impl BasaltApp {
    pub fn new(cc: &eframe::CreationContext<'_>, init: AppInit) -> Self {
        let config = init.config.clone();
        let runtime_handle = init.runtime_handle.clone();

        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let shared = Arc::new(AppState::with_event_channel(event_tx));
        let _ = state::install_global(shared.clone());

        let rest = Arc::new(Http::new(config.plain_token()).expect("rest client init"));
        if let Some(t) = config.plain_token() {
            crate::scrub::set_live_token(&t);
            rest.set_token(t);
        }
        let _ = crate::rest::install_global(rest.clone());

        let gateway = Arc::new(Gateway::new(shared.clone()));
        let gateway_tx = gateway.sender();
        gateway.clone().spawn();

        crate::ui::apply_dark_theme(&cc.egui_ctx, config.use_legacy_status_dots);

        if let Some(token) = config.plain_token() {
            tracing::info!("token present - auto-connecting gateway + fetching user");
            let rest_clone = rest.clone();
            let shared_clone = shared.clone();
            let gateway_tx_clone = gateway_tx.clone();
            let dm_seed: Vec<u64> = config
                .dm_channel_ids
                .iter()
                .filter_map(|id| id.parse().ok())
                .collect();
            let mut dm_seed_alive = dm_seed.clone();
            runtime_handle.spawn(async move {
                // Probe the token type first (raw user-token auth vs the
                // Bot prefix) so the gateway gets the right IDENTIFY shape.
                let mut result = rest_clone.get_current_user().await;
                let mut is_bot = false;
                if let Err(crate::rest::HttpError::Discord(code, _)) = &result {
                    if code.as_u16() == 401 {
                        // Probably a bot token - retry with the Bot prefix.
                        rest_clone.set_bot_prefix(true);
                        result = rest_clone.get_current_user().await;
                        is_bot = result.is_ok();
                    }
                }
                match result {
                    Ok(u) => {
                        *shared_clone.current_user.write() = Some(u);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "REST get_current_user failed");
                    }
                }
                let _ = gateway_tx_clone.send(Outbound::Connect { token, bot: is_bot });
                match rest_clone.get_my_guilds(true).await {
                    Ok(g) => {
                        *shared_clone.guilds.write() = g;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "REST get_my_guilds failed");
                    }
                }
                // Also warm the DM list (REST covers what READY may not
                // give bot accounts).
                match rest_clone.get_my_dm_channels().await {
                    Ok(dms) => {
                        let mut ch = shared_clone.channels.write();
                        for c in dms {
                            if !ch.iter().any(|x| x.id == c.id) {
                                ch.push(c);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "REST get dm channels failed");
                    }
                }
                // Seed DMs remembered from past sessions: bot accounts have
                // no other way to recover their DM list after a restart.
                for id in dm_seed.clone() {
                    match rest_clone.get_channel(crate::model::Snowflake(id)).await {
                        Ok(c) => {
                            let mut ch = shared_clone.channels.write();
                            if !ch.iter().any(|x| x.id == c.id) {
                                ch.push(c);
                            }
                        }
                        Err(_) => {
                            // Closed or deleted DM: drop it from the seed.
                            dm_seed_alive.retain(|&v| v != id);
                        }
                    }
                }
            });
        }

        // Session mirrors of config lists + the startup notice (updater).
        {
            let shared = shared.clone();
            let dm_pins = config.pinned_dms.clone();
            runtime_handle.spawn_blocking(move || {
                shared.init_pinned_dms(&dm_pins);
            });
        }
        let startup_notice = config.startup_notice.clone();
        if let Some(notice) = startup_notice {
            crate::ui::toast::success(notice);
        }

        let (send_tx, send_rx) = crate::sender::channel();
        let rest_worker = rest.clone();
        runtime_handle.spawn(async move {
            // The worker owns the receiving half for the whole session.
            crate::sender::spawn_worker(rest_worker, send_rx);
        });

        let last_legacy_dots = config.use_legacy_status_dots;
        Self {
            send_queue: send_tx,
            config,
            shared,
            rest,
            gateway_tx,
            _gateway_task: Some(gateway),
            event_rx,
            chat: ChatState::default(),
            login: LoginState::default(),
            settings: SettingsState::default(),
            first_frame: true,
            auto_selected: false,
            last_legacy_dots,
            runtime_handle,
        }
    }

    /// Sign in from the login screen: probe the token type, connect the
    /// gateway, fetch user, guilds, DMs.
    fn start_session(&mut self, token: String) {
        crate::scrub::set_live_token(&token);
        self.config.set_plain_token(&token);
        let _ = self.config.save();
        self.rest.set_token(token.clone());
        let rest = self.rest.clone();
        let shared = self.shared.clone();
        let gateway_tx = self.gateway_tx.clone();
        self.runtime_handle.spawn(async move {
            let mut me = rest.get_current_user().await;
            let mut is_bot = false;
            if let Err(crate::rest::HttpError::Discord(code, _)) = &me {
                if code.as_u16() == 401 {
                    rest.set_bot_prefix(true);
                    me = rest.get_current_user().await;
                    is_bot = me.is_ok();
                }
            }
            if let Ok(u) = me {
                *shared.current_user.write() = Some(u);
            }
            let _ = gateway_tx.send(Outbound::Connect { token, bot: is_bot });
            if let Ok(g) = rest.get_my_guilds(true).await {
                *shared.guilds.write() = g;
            }
            if let Ok(dms) = rest.get_my_dm_channels().await {
                let mut ch = shared.channels.write();
                for c in dms {
                    if !ch.iter().any(|x| x.id == c.id) {
                        ch.push(c);
                    }
                }
            }
        });
    }

    /// Sign out: clear the token from config + state, disconnect gateway.
    fn sign_out(&mut self) {
        self.config.clear_token();
        let _ = self.config.save();
        self.rest.clear_token();
        let _ = self.gateway_tx.send(Outbound::Shutdown);
        *self.shared.current_user.write() = None;
        self.shared.guilds.write().clear();
        self.shared.channels.write().clear();
        self.chat = ChatState::default();
        self.config.dm_channel_ids.clear();
        let _ = self.config.save();
        // Restart the gateway task so a future sign-in can connect.
        let gateway = Arc::new(Gateway::new(self.shared.clone()));
        self.gateway_tx = gateway.sender();
        gateway.clone().spawn();
        self._gateway_task = Some(gateway);
    }

    /// Pump gateway events that arrived since last frame. Events mutate the
    /// shared state in `dispatch_event` (called on the gateway task); this
    /// drain forwards internal UI commands (presence requests) to the
    /// gateway and requests repaints when something happened.
    fn pump_events(&mut self, ctx: &egui::Context) {
        let mut got_any = false;
        let mut new_dm: Option<u64> = None;
        while let Ok(event) = self.event_rx.try_recv() {
            got_any = true;
            match event {
                crate::gateway::events::Event::PresenceRequested { status } => {
                    let _ = self
                        .gateway_tx
                        .send(crate::gateway::Outbound::SetPresence { status, afk: false });
                }
                // Remember newly seen DM channels so the list survives
                // restarts (bots get no DM list from the API).
                crate::gateway::events::Event::MessageCreate { d } => {
                    let cid_str = d.channel_id.0.to_string();
                    if d.guild_id.is_none() && !self.config.dm_channel_ids.contains(&cid_str) {
                        self.config.dm_channel_ids.push(cid_str);
                        new_dm = Some(d.channel_id.0);
                    }
                }
                _ => {}
            }
        }
        if let Some(id) = new_dm {
            let _ = self.config.save();
            let _ = id;
        }
        if got_any {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for BasaltApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.first_frame {
            crate::ui::apply_dark_theme(&ctx, self.config.use_legacy_status_dots);
            self.first_frame = false;
        }
        // Re-apply theme when the legacy-status-dots toggle changes.
        if self.last_legacy_dots != self.config.use_legacy_status_dots {
            crate::ui::apply_dark_theme(&ctx, self.config.use_legacy_status_dots);
            self.last_legacy_dots = self.config.use_legacy_status_dots;
        }
        // Repaint budget: snappy enough for typing indicators, cheap at idle.
        ctx.request_repaint_after(std::time::Duration::from_millis(120));

        // Notification prefs (sound / banners) mirror into the notify module.
        crate::notify::set_prefs(self.config.notification_sounds, self.config.desktop_notifications);

        self.pump_events(&ctx);

        // One-shot auto-selection of the first guild + channel. Retries
        // every frame until GUILD_CREATE delivers the channel list.
        if !self.auto_selected && self.shared.current_user().is_some() {
            let has_guilds = !self.shared.guilds.read().is_empty();
            let sel = self.shared.selection_sync();
            if has_guilds && sel.guild_id.is_none() && sel.channel_id.is_none() {
                let first_guild = self.shared.guilds.read()[0].id;
                self.shared.set_selection_sync(Selection {
                    guild_id: Some(first_guild),
                    channel_id: None,
                });
                // Also ask REST for the channel list (covers slow events).
                crate::ui::guilds_bar::fetch_guild_channels(self.rest.clone(), first_guild);
            }
            if sel.channel_id.is_some() || self.shared.selection_sync().channel_id.is_some() {
                let sel = self.shared.selection_sync();
                if let Some(cid) = sel.channel_id {
                    if !self.shared.is_fetched(cid) {
                        crate::ui::sidebar::fetch_channel_messages(&self.rest, &self.shared, cid);
                    }
                    self.auto_selected = true;
                }
            }
        }

        let sel_now = self.shared.selection_sync();
        let group_dm = sel_now
            .channel_id
            .and_then(|cid| self.shared.channel_by_id(cid))
            .map(|c| matches!(c.kind, crate::model::ChannelType::GroupDm))
            .unwrap_or(false);
        let show_members = self.config.show_members
            && (sel_now.guild_id.is_some() || group_dm)
            && self.shared.current_user().is_some();

        // ── Right: member list (resizable, width persisted) ──
        if show_members {
            let shared = self.shared.clone();
            let group_dm_v = group_dm;
            egui::Panel::right("members")
                .resizable(true)
                .default_size(self.config.members_width)
                .min_size(180.0)
                .max_size(360.0)
                .frame(egui::Frame::new().fill(colors::BG_SIDEBAR).inner_margin(egui::Margin::same(0)))
                .show_separator_line(false)
                .show(ui, |ui| {
                    if group_dm_v {
                        crate::ui::members::render_group_dm(ui, &shared);
                    } else {
                        crate::ui::members::render(ui, &shared);
                    }
                    let w = ui.min_rect().width();
                    let saved = self.config.members_width;
                    if (w - saved).abs() > 1.0 && (180.0..=360.0).contains(&w) {
                        self.config.members_width = w;
                    }
                });
        }

        // ── Left: guilds bar (72px, full height) ──
        {
            let shared = self.shared.clone();
            let rest = self.rest.clone();
            let config = &mut self.config;
            let new_sel = egui::Panel::left("guilds")
                .exact_size(72.0)
                .frame(egui::Frame::new().fill(colors::BG_GUILDS_BAR).inner_margin(egui::Margin::same(0)))
                .show_separator_line(false)
                .show(ui, |ui| {
                    crate::ui::guilds_bar::render(ui, &shared, rest, config)
                })
                .inner;
            if let Some(sel) = new_sel {
                self.shared.set_selection_sync(sel);
                // Keyboard focus follows the conversation: Discord keeps
                // the composer focused after picking a server/channel, so
                // typing right after a click goes into the message box.
                self.chat.want_composer_focus = true;
            }
        }

        // ── Left: sidebar (resizable, full height, width persisted) ──
        let sidebar_w = self.config.sidebar_width;
        {
            let shared = self.shared.clone();
            let rest = self.rest.clone();
            let config = &mut self.config;
            let new_sel = egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(sidebar_w)
                .min_size(200.0)
                .max_size(360.0)
                .frame(egui::Frame::new().fill(colors::BG_SIDEBAR).inner_margin(egui::Margin::same(0)))
                .show_separator_line(false)
                .show(ui, |ui| {
                    let out = crate::ui::sidebar::render(ui, &shared, rest, config);
                    // Persist the dragged width (throttled: only when it
                    // actually changed by a pixel or more).
                    let w = ui.min_rect().width();
                    let saved = config.sidebar_width;
                    if (w - saved).abs() > 1.0 && (200.0..=360.0).contains(&w) {
                        config.sidebar_width = w;
                    }
                    out
                })
                .inner;
            let saved_key = egui::Id::new("sidebar_saved_w");
            let saved: f32 = ctx
                .data(|d| d.get_temp::<f32>(saved_key))
                .unwrap_or(self.config.sidebar_width);
            if (self.config.sidebar_width - saved).abs() > 1.0 {
                let _ = self.config.save();
                ctx.data_mut(|d| d.insert_temp(saved_key, self.config.sidebar_width));
            }
            if let Some(sel) = new_sel {
                // Selecting a channel: fetch its history if needed.
                let new_sel = sel;
                let needs_fetch = new_sel
                    .channel_id
                    .map(|cid| !self.shared.is_fetched(cid))
                    .unwrap_or(false);
                self.shared.set_selection_sync(new_sel.clone());
                // Keep the composer focused after a channel switch (Discord
                // behavior); without this the click leaves keyboard focus
                // on the row and typed messages go nowhere.
                self.chat.want_composer_focus = true;
                if needs_fetch {
                    if let Some(cid) = new_sel.channel_id {
                        crate::ui::sidebar::fetch_channel_messages(&self.rest, &self.shared, cid);
                    }
                }
            }
        }

        // ── Center: chat or login (fills the rest of the window) ──
        let mut start_token: Option<String> = None;
        {
            let shared = self.shared.clone();
            let rest = self.rest.clone();
            let sender = self.send_queue.clone();
            let chat = &mut self.chat;
            let config = &mut self.config;
            let login = &mut self.login;
            let start_token_ref = &mut start_token;
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(colors::BG_CHAT).inner_margin(egui::Margin::same(0)))
                .show(ui, |ui| {
                    if shared.current_user().is_some() {
                        crate::ui::chat::render(ui, &shared, rest, &sender, chat, config);
                    } else {
                        let just_signed_in = crate::ui::login::render(ui, login, config);
                        if just_signed_in {
                            if let Some(token) = config.plain_token() {
                                *start_token_ref = Some(token);
                            }
                        }
                    }
                });
        }
        if let Some(token) = start_token {
            self.start_session(token);
        }

        // ── Settings modal (overlay, above everything) ──
        // Only fire the open() transition (which resets the slide-in
        // animation) on the closed→open edge; calling it every frame used to
        // reset the animation each frame and fight the close handler.
        if self.shared.settings_open() && !self.settings.open {
            self.settings.open();
        }
        let mut sign_out_requested = false;
        if self.settings.open {
            // Split mut borrows so the closure can take both.
            let settings = &mut self.settings;
            let config = &mut self.config;
            let shared = &self.shared;
            let gateway_tx = self.gateway_tx.clone();
            let sign_out = &mut sign_out_requested;
            egui::Area::new(egui::Id::new("settings_modal"))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    ui.allocate_ui_with_layout(
                        ctx.viewport_rect().size(),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            *sign_out = crate::ui::settings::render(
                                ui,
                                settings,
                                config,
                                shared,
                                &gateway_tx,
                            );
                        },
                    );
                });
        }
        if sign_out_requested {
            self.sign_out();
        }

        // Toasts (copies, update progress, notices) render above all.
        crate::ui::toast::render(&ctx);

        // ── Connection banner (bottom of chat, Discord-style) ──
        let status = self.shared.connection_status_sync();
        if matches!(
            status,
            state::ConnectionStatus::Disconnected
                | state::ConnectionStatus::Connecting
                | state::ConnectionStatus::Reconnecting
                | state::ConnectionStatus::AuthFailed
        ) && self.shared.current_user().is_some()
        {
            let label = crate::ui::connection_label(status);
            let color = match status {
                state::ConnectionStatus::AuthFailed => colors::RED,
                _ => colors::STATUS_IDLE,
            };
            let side_w = self.config.sidebar_width;
            let banner_rect = egui::Rect::from_min_size(
                ctx.viewport_rect().min + egui::vec2(72.0 + side_w, ctx.viewport_rect().height() - 28.0),
                egui::vec2(ctx.viewport_rect().width() - 72.0 - side_w, 28.0),
            );
            let painter = ctx.layer_painter(egui::LayerId::background());
            painter.rect_filled(banner_rect, 0.0, colors::BG_FLOATING);
            painter.text(
                banner_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(13.0),
                color,
            );
        }
    }
}
