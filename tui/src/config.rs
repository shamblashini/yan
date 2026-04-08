use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Unique identity for this device. Generated once and never changes.
    pub device_id: Uuid,
    /// Base URL of the yan sync server, e.g. "https://yan.example.com".
    /// Leave empty or absent to run in offline-only mode.
    #[serde(default)]
    pub server_url: String,
    /// Bearer token for server authentication.
    #[serde(default)]
    pub auth_token: String,
    /// Whether sync is enabled. Requires server_url and auth_token.
    #[serde(default)]
    pub sync_enabled: bool,
}

impl Config {
    pub fn is_sync_configured(&self) -> bool {
        self.sync_enabled && !self.server_url.is_empty() && !self.auth_token.is_empty()
    }
}

pub fn config_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
    });
    base.join("yan").join("config.toml")
}

pub fn load() -> Config {
    let path = config_path();
    if path.exists() {
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(cfg) = toml::from_str::<Config>(&contents) {
                return cfg;
            }
        }
    }
    // First run: generate a device_id and save it.
    let cfg = Config {
        device_id: Uuid::new_v4(),
        server_url: String::new(),
        auth_token: String::new(),
        sync_enabled: false,
    };
    let _ = save(&cfg);
    cfg
}

/// Interactive config wizard. Runs on stdin/stdout before the TUI starts.
/// Invoked via `yan-tui --config`.
pub fn run_wizard() {
    let mut cfg = load();
    let path = config_path();

    println!("yan sync configuration");
    println!("Config file: {}", path.display());
    println!("(Leave blank to keep existing value)\n");

    let current_url = if cfg.server_url.is_empty() {
        "none".to_string()
    } else {
        cfg.server_url.clone()
    };
    print!("Backend URL [{}]: ", current_url);
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if !input.is_empty() {
        cfg.server_url = input.to_string();
    }

    let current_token = if cfg.auth_token.is_empty() {
        "none".to_string()
    } else {
        format!("{}***", &cfg.auth_token[..cfg.auth_token.len().min(4)])
    };
    print!("API token [{}]: ", current_token);
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if !input.is_empty() {
        cfg.auth_token = input.to_string();
    }

    let can_sync = !cfg.server_url.is_empty() && !cfg.auth_token.is_empty();
    if can_sync {
        let current_enabled = if cfg.sync_enabled { "Y/n" } else { "y/N" };
        print!("Enable sync? [{}]: ", current_enabled);
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim().to_lowercase();
        cfg.sync_enabled = match input.as_str() {
            "n" | "no" => false,
            "y" | "yes" => true,
            "" => cfg.sync_enabled,
            _ => cfg.sync_enabled,
        };
    } else {
        cfg.sync_enabled = false;
        if !cfg.server_url.is_empty() || !cfg.auth_token.is_empty() {
            println!("Sync disabled: both URL and token are required.");
        }
    }

    match save(&cfg) {
        Ok(()) => println!("\nConfig saved to {}", path.display()),
        Err(e) => eprintln!("\nFailed to save config: {e}"),
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    fs::write(&path, contents)?;
    Ok(())
}
