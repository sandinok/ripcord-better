//! CLI flag parsing for basalt.

use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct Args {
    /// Override the stored token. Accepts a literal token OR a path to a
    /// file containing a token (`@<path>`). Useful for `rb --token @-`
    /// piped from a password manager.
    pub token: Option<String>,
    /// Override config file path (default: `~/.config/basalt/config.toml`).
    pub config: Option<PathBuf>,
    /// Print version + binary size + Cargo profile and exit.
    pub print_version: bool,
    /// Verbose logging (`-vv` for trace).
    pub verbose: u8,
}

pub fn parse() -> Result<Args, String> {
    let mut a = Args::default();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "-V" | "--version" => {
                a.print_version = true;
            }
            "-v" => a.verbose = 1,
            "-vv" => a.verbose = 2,
            "-t" | "--token" => {
                i += 1;
                if i >= raw.len() {
                    return Err(format!("{:?} requires an argument", raw[i - 1]));
                }
                a.token = Some(parse_token_arg(&raw[i])?);
            }
            "-c" | "--config" => {
                i += 1;
                if i >= raw.len() {
                    return Err(format!("{:?} requires an argument", raw[i - 1]));
                }
                a.config = Some(PathBuf::from(&raw[i]));
            }
            other if other.starts_with("--token=") => {
                a.token = Some(parse_token_arg(other.trim_start_matches("--token="))?);
            }
            other if other.starts_with("--config=") => {
                a.config = Some(PathBuf::from(other.trim_start_matches("--config=")));
            }
            other => {
                return Err(format!("unknown argument: {other}"));
            }
        }
        i += 1;
    }
    Ok(a)
}

fn parse_token_arg(s: &str) -> Result<String, String> {
    if let Some(path) = s.strip_prefix('@') {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("reading token file {path}: {e}"))?;
        Ok(raw.trim().to_string())
    } else {
        Ok(s.to_string())
    }
}

fn print_help() {
    println!(
        "basalt {version}\n\
         Native Rust+egui Discord client. No WebView2, no Electron.\n\
         \n\
         USAGE:\n  basalt [OPTIONS]\n\
         \n\
         OPTIONS:\n  \
         -h, --help                  Print this help and exit\n  \
         -V, --version               Print version + binary metadata and exit\n  \
         -v / -vv                    Verbose logging (info / trace)\n  \
         -t, --token <T|@PATH>      Use this token (or @file) instead of the stored one\n  \
         -c, --config PATH          Use this config file instead of the default\n\
         \n\
         ENV:\n  \
         DISCORD_TOKEN               Shorthand for --token\n",
        version = env!("CARGO_PKG_VERSION")
    );
}
