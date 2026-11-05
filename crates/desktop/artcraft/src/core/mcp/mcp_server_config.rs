//! Port and access token for the local MCP server.
//!
//! The token is created once and kept in `<data dir>/credentials/mcp_server.json`,
//! so MCP client configurations stay valid across app restarts. The same file also
//! tells the stdio bridge (`script/artcraft/artcraft_mcp_bridge.mjs`) where to connect.

use std::fs;
use std::path::{Path, PathBuf};

use errors::AnyhowResult;
use log::warn;
use serde_derive::{Deserialize, Serialize};
use uuid_utils::uuid::generate_random_uuid;

use crate::core::state::data_dir::app_data_root::AppDataRoot;
use crate::core::state::data_dir::trait_data_subdir::DataSubdir;

pub const DEFAULT_MCP_PORT: u16 = 41950;

const CONFIG_FILE_NAME: &str = "mcp_server.json";
const PORT_ENV_VAR: &str = "ARTCRAFT_MCP_PORT";
const DISABLED_ENV_VAR: &str = "ARTCRAFT_MCP_DISABLED";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpServerConfig {
  /// Endpoint MCP clients connect to, e.g. `http://127.0.0.1:41950/mcp`.
  pub url: String,

  pub port: u16,

  /// Secret sent as `Authorization: Bearer <token>`.
  pub token: String,
}

impl McpServerConfig {
  /// Load the stored token (creating it on first run) and write the current port and URL.
  pub fn load_or_create(root: &AppDataRoot) -> AnyhowResult<Self> {
    let path = config_path(root);
    let port = port_from_env().unwrap_or(DEFAULT_MCP_PORT);

    let token = read_existing(&path)
        .map(|existing| existing.token)
        .filter(|token| token.len() >= 32)
        .unwrap_or_else(new_token);

    let config = Self::new(port, token);
    write_private_file(&path, &serde_json::to_string_pretty(&config)?)?;
    Ok(config)
  }

  pub fn new(port: u16, token: String) -> Self {
    Self {
      url: format!("http://127.0.0.1:{}/mcp", port),
      port,
      token,
    }
  }

  pub fn is_disabled_by_env() -> bool {
    std::env::var(DISABLED_ENV_VAR)
        .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
  }
}

pub fn config_path(root: &AppDataRoot) -> PathBuf {
  root.credentials_dir().path().join(CONFIG_FILE_NAME)
}

fn port_from_env() -> Option<u16> {
  let value = std::env::var(PORT_ENV_VAR).ok()?;
  match value.trim().parse::<u16>() {
    Ok(port) if port > 0 => Some(port),
    _ => {
      warn!("Ignoring invalid {}={:?}", PORT_ENV_VAR, value);
      None
    }
  }
}

fn read_existing(path: &Path) -> Option<McpServerConfig> {
  let contents = fs::read_to_string(path).ok()?;
  serde_json::from_str(&contents).ok()
}

fn new_token() -> String {
  format!("{}{}", generate_random_uuid(), generate_random_uuid()).replace('-', "")
}

fn write_private_file(path: &Path, contents: &str) -> AnyhowResult<()> {
  fs::write(path, contents)?;

  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn url_uses_loopback_and_port() {
    let config = McpServerConfig::new(5000, "t".repeat(32));
    assert_eq!(config.url, "http://127.0.0.1:5000/mcp");
  }

  #[test]
  fn new_token_is_long_and_unique() {
    let first = new_token();
    assert!(first.len() >= 32);
    assert!(!first.contains('-'));
    assert_ne!(first, new_token());
  }
}
