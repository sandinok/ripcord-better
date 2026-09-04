//! Basalt - main entry point.
//!
//! A native Rust + egui Discord client. No WebView2, no Electron.
//! Basalt: dark, dense, columnar - like the rock it's named after.

// Basalt is a v0.1 beta; some code paths are scaffold for future features
// (voice, presence, reaction add/remove, ...). Suppress dead-code lints
// project-wide so they don't drown out real warnings.
#![allow(dead_code, clippy::wrong_self_convention)]
// Release builds on Windows run as a pure GUI app: no console window
// flashing behind the UI. Debug builds keep the console for tracing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// `time::format_description::parse` is marked deprecated in time 0.3.41+ in
// favor of `parse_borrowed`, but `parse_borrowed` only exists on the
// `well_known` family. Our use of `parse("[hour]:[minute]")` works on the
// current API. Suppress the deprecation until `time` ships a real
// replacement.
#![allow(deprecated)]
// In tests we use `let mut x = T::default(); x.field = value;` for
// readability — easier than writing full struct-literal initializers
// when we only care about one or two fields. Suppress the stylistic
// `field_reassign_with_default` lint in test code.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

mod app;
mod colors;
mod config;
mod gateway;
mod icons;
mod identity;
mod icons_data;
mod image_loader;
mod markdown;
mod model;
mod notify;
mod rest;
mod scrub;
mod sender;
mod state;
mod ui;
mod updater;

use std::process::ExitCode;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Window icon embedded at compile time. 32x32 PNG, ~750 bytes.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon-32.png");

fn main() -> ExitCode {
    let args = match config::cli::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    // Parse CLI flags first so -v / -vv can shape the filter, then init
    // logging. Quiet by default: a GUI app talks through its window, not
    // through the terminal. RUST_LOG always wins if the user sets it.
    init_logging(args.verbose);
    // Every panic line goes through the redactor too (point 3).
    scrub::install_panic_hook();

    if args.print_version {
        println!(
            "basalt {}\ntarget: {}-{}\ncargo profile: release (opt-level=z, lto=fat)\nrust toolchain: {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            rustc_version(),
        );
        return ExitCode::SUCCESS;
    }

    let mut cfg = match config::Config::load(&args) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to load config");
            eprintln!("basalt: config error: {e}");
            return ExitCode::from(3);
        }
    };

    // Updater lifecycle: --post-update verifies the swap and cleans the
    // backup; --update-failed restores the old binary's trail; a stale
    // marker from an update that never started is tidied up too.
    if args.post_update {
        if let Some(msg) = updater::post_update_check() {
            cfg.startup_notice = Some(msg);
        }
    } else if args.update_failed {
        if let Some(msg) = updater::cleanup_stale_update() {
            cfg.startup_notice = Some(msg);
        }
    } else if let Some(msg) = updater::cleanup_stale_update() {
        cfg.startup_notice = Some(msg);
    }

    // Two async workers: one is enough for REST, but the gateway heartbeats
    // must never queue behind a busy worker (a stalled heartbeat is how
    // "Connection lost" sessions die). Image decode runs on the blocking
    // pool (see image_loader), so 2 + 8 threads stays ~10 MB of stacks.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .enable_all()
        .thread_name("basalt-worker")
        .thread_stack_size(256 * 1024)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "failed to spawn tokio runtime");
            return ExitCode::from(4);
        }
    };
    let runtime = Box::leak(Box::new(runtime));
    // Keep the tokio context installed on the main (GUI) thread so UI code
    // can `tokio::spawn` fetch tasks directly. The guard is intentionally
    // leaked: it must outlive the app.
    std::mem::forget(runtime.enter());

    let icon = decode_window_icon();

    let viewport = egui::ViewportBuilder::default()
        .with_title("Basalt")
        .with_inner_size([1200.0, 800.0])
        .with_min_inner_size([800.0, 600.0])
        .with_active(true);

    let viewport = if let Some(ic) = icon {
        viewport.with_icon(std::sync::Arc::new(ic))
    } else {
        viewport
    };

    let native_opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app_init = app::AppInit {
        config: cfg,
        runtime_handle: runtime.handle().clone(),
    };
    if let Err(e) = eframe::run_native(
        "basalt",
        native_opts,
        Box::new(move |cc| Ok(Box::new(app::BasaltApp::new(cc, app_init)))),
    ) {
        tracing::error!(error = %e, "eframe::run_native failed");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn rustc_version() -> &'static str {
    option_env!("RUSTC_VERSION").unwrap_or("stable")
}

/// Set up `tracing`. Level ladder: default `warn`, `-v` info, `-vv` trace.
/// `RUST_LOG` (if exported by the user) overrides everything.
///
/// Every event is formatted through [`scrub::ScrubbedFormatter`] so no
/// token-shaped string ever reaches stderr.
fn init_logging(verbosity: u8) {
    let level = match verbosity {
        0 => "warn",
        1 => "info",
        _ => "trace",
    };
    let env = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{level},hyper=warn,reqwest=warn,tungstenite=warn")));
    tracing_subscriber::registry()
        .with(env)
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .event_format(scrub::ScrubbedFormatter),
        )
        .init();
}

/// Decode the embedded PNG into an `egui::IconData` for window chrome.
/// Falls back to `None` if the decode fails (which should never happen
/// for a known-good embedded asset, but we defend anyway).
fn decode_window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}
