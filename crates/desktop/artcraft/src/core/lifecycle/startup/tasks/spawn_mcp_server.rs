use log::{error, info};
use tauri::AppHandle;

use crate::core::mcp::mcp_http_server::run_mcp_http_server;
use crate::core::mcp::mcp_server_config::McpServerConfig;
use crate::core::mcp::mcp_tauri_executor::McpTauriExecutor;
use crate::core::state::data_dir::app_data_root::AppDataRoot;

/// Start the local MCP server. Failures are logged and never block app startup.
/// Runs after the task database is bootstrapped, which generation needs.
pub fn spawn_mcp_server(app: &AppHandle, root: &AppDataRoot) {
  if McpServerConfig::is_disabled_by_env() {
    info!("MCP server disabled by environment.");
    return;
  }

  let config = match McpServerConfig::load_or_create(root) {
    Ok(config) => config,
    Err(err) => {
      error!("Could not prepare MCP server config: {:?}", err);
      return;
    }
  };

  let executor = McpTauriExecutor::new(app.clone());

  tauri::async_runtime::spawn(async move {
    if let Err(err) = run_mcp_http_server(config, executor).await {
      error!("MCP server stopped: {:?}", err);
    }
  });
}
