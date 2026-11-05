//! Runs MCP tool calls against the running desktop app.
//!
//! Every call goes through the same Tauri command code the UI uses, so MCP
//! generations use the signed-in account, are billed the same way, and show up
//! in the app's task queue and library.

use std::future::Future;
use std::time::{Duration, Instant};

use artcraft_api_defs::jobs::list_session_jobs::ListSessionJobsItem;
use artcraft_client::endpoints::jobs::list_session_jobs::{list_session_jobs, States};
use enums::common::job_status_plus::JobStatusPlus;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

use crate::core::commands::cost_estimate::estimate_image_cost_command::estimate_image_cost_command;
use crate::core::commands::cost_estimate::estimate_video_cost_command::estimate_video_cost_command;
use crate::core::commands::generate::generate_image::generate_image_command::generate_image_command;
use crate::core::commands::generate::generate_video::generate_video_command::generate_video_command;
use crate::core::commands::generate::models::image::list_image_models_command::list_image_models_command;
use crate::core::commands::generate::models::video::list_video_models_command::list_video_models_command;
use crate::core::commands::generate::omni::OmniRequest;
use crate::core::commands::response::failure_response_wrapper::CommandErrorResponseWrapper;
use crate::core::commands::response::success_response_wrapper::CommandSuccessResponseWrapper;
use crate::core::mcp::mcp_protocol::{McpToolExecutor, ToolOutcome};
use crate::core::mcp::mcp_tools::{McpToolCall, MediaKind};
use crate::core::state::app_env_configs::app_env_configs::AppEnvConfigs;
use crate::services::storyteller::commands::storyteller_get_credits_command::storyteller_get_credits_command;
use crate::services::storyteller::state::storyteller_credential_manager::StorytellerCredentialManager;

const JOB_POLL_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct McpTauriExecutor {
  app: AppHandle,
}

impl McpTauriExecutor {
  pub fn new(app: AppHandle) -> Self {
    Self { app }
  }

  async fn run(&self, call: McpToolCall) -> ToolOutcome {
    match call {
      McpToolCall::ListModels { kind } => self.list_models(kind).await,
      McpToolCall::EstimateCost { kind, fields } => self.estimate_cost(kind, fields).await,
      McpToolCall::Generate { kind, fields } => self.generate(kind, fields).await,
      McpToolCall::GetJobs { job_tokens, wait_seconds } => self.get_jobs(&job_tokens, wait_seconds).await,
      McpToolCall::GetCredits => {
        command_outcome(storyteller_get_credits_command(self.app.state(), self.app.state()).await)
      }
    }
  }

  async fn list_models(&self, kind: MediaKind) -> ToolOutcome {
    match kind {
      MediaKind::Image => command_outcome(list_image_models_command(self.app.state()).await),
      MediaKind::Video => command_outcome(list_video_models_command(self.app.state()).await),
    }
  }

  async fn estimate_cost(&self, kind: MediaKind, fields: Map<String, Value>) -> ToolOutcome {
    let request = match omni_request(fields) {
      Ok(request) => request,
      Err(outcome) => return outcome,
    };
    let app = self.app.clone();
    match kind {
      MediaKind::Image => command_outcome(estimate_image_cost_command(request, app).await),
      MediaKind::Video => command_outcome(estimate_video_cost_command(request, app).await),
    }
  }

  async fn generate(&self, kind: MediaKind, fields: Map<String, Value>) -> ToolOutcome {
    let request = match omni_request(fields) {
      Ok(request) => request,
      Err(outcome) => return outcome,
    };
    let app = self.app.clone();
    let result = match kind {
      MediaKind::Image => generate_image_command(request, app).await,
      MediaKind::Video => generate_video_command(request, app).await,
    };

    match result {
      Ok(success) => {
        let payload = success.payload.map(|payload| payload.0).unwrap_or(Value::Null);
        let job_tokens = job_tokens(&payload);
        ToolOutcome::ok(&json!({
          "job_tokens": job_tokens,
          "next_step": "Call artcraft_get_jobs with these job_tokens and wait_seconds to get the result.",
          "response": payload,
        }))
      }
      Err(err) => error_outcome(err),
    }
  }

  async fn get_jobs(&self, job_tokens: &[String], wait_seconds: u64) -> ToolOutcome {
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);

    loop {
      let jobs = match self.fetch_jobs(job_tokens).await {
        Ok(jobs) => jobs,
        Err(message) => return ToolOutcome::error(message),
      };

      let all_done = jobs.iter().all(|job| job.as_ref().is_none_or(is_finished));
      if all_done || Instant::now() + JOB_POLL_INTERVAL > deadline {
        let summaries: Vec<Value> = job_tokens.iter()
            .zip(jobs.iter())
            .map(|(token, job)| summarize_job(token, job.as_ref()))
            .collect();
        return ToolOutcome::ok(&json!({ "jobs": summaries, "all_finished": all_done }));
      }

      tokio::time::sleep(JOB_POLL_INTERVAL).await;
    }
  }

  /// One entry per requested token, `None` when the session doesn't know the job.
  async fn fetch_jobs(&self, job_tokens: &[String]) -> Result<Vec<Option<ListSessionJobsItem>>, String> {
    let configs = self.app.state::<AppEnvConfigs>();
    let creds = self.app.state::<StorytellerCredentialManager>()
        .get_credentials()
        .map_err(|err| format!("Could not read ArtCraft credentials: {}", err))?
        .ok_or_else(|| "Not logged into ArtCraft. Log in from the ArtCraft app and try again.".to_string())?;

    let response = list_session_jobs(&configs.storyteller_host, Some(&creds), States::All)
        .await
        .map_err(|err| format!("Could not fetch ArtCraft jobs: {}", err))?;

    let mut jobs: Vec<Option<ListSessionJobsItem>> = Vec::with_capacity(job_tokens.len());
    let mut remaining = response.jobs;
    for token in job_tokens {
      let index = remaining.iter().position(|job| job.job_token.as_str() == token);
      jobs.push(index.map(|index| remaining.swap_remove(index)));
    }
    Ok(jobs)
  }
}

impl McpToolExecutor for McpTauriExecutor {
  fn execute(&self, call: McpToolCall) -> impl Future<Output = ToolOutcome> + Send {
    let executor = self.clone();
    async move { executor.run(call).await }
  }
}

fn omni_request(fields: Map<String, Value>) -> Result<OmniRequest, ToolOutcome> {
  serde_json::from_value(Value::Object(fields))
      .map_err(|err| ToolOutcome::error(format!("Invalid generation options: {}", err)))
}

fn summarize_job(token: &str, job: Option<&ListSessionJobsItem>) -> Value {
  let Some(job) = job else {
    return json!({ "job_token": token, "status": "not_found" });
  };

  let mut summary = json!({
    "job_token": token,
    "status": job.status.status.to_str(),
    "progress_percentage": job.status.progress_percentage,
  });

  if let Some(message) = job.status.maybe_failure_message.as_deref() {
    summary["failure_message"] = json!(message);
  }

  if let Some(result) = job.maybe_result.as_ref() {
    summary["result"] = json!({
      "url": result.media_links.cdn_url.as_str(),
      "media_file_token": result.entity_token,
      "entity_type": result.entity_type,
      "thumbnail_url_template": result.media_links.maybe_thumbnail_template,
    });
  }

  summary
}

fn is_finished(job: &ListSessionJobsItem) -> bool {
  matches!(
    job.status.status,
    JobStatusPlus::CompleteSuccess
      | JobStatusPlus::CompleteFailure
      | JobStatusPlus::Dead
      | JobStatusPlus::CancelledByUser
      | JobStatusPlus::CancelledBySystem
  )
}

fn job_tokens(response: &Value) -> Vec<String> {
  let mut tokens: Vec<String> = Vec::new();
  let primary = response.get("inference_job_token").and_then(Value::as_str);
  let all = response.get("all_job_tokens").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
  for token in primary.into_iter().chain(all) {
    if !tokens.iter().any(|existing| existing == token) {
      tokens.push(token.to_owned());
    }
  }
  tokens
}

fn command_outcome<T: Serialize, E: Serialize, P: Serialize>(
  result: Result<CommandSuccessResponseWrapper<T>, CommandErrorResponseWrapper<E, P>>,
) -> ToolOutcome {
  match result {
    Ok(success) => match serde_json::to_value(&success.payload) {
      Ok(value) => ToolOutcome::ok(&value),
      Err(err) => ToolOutcome::error(format!("Could not encode result: {}", err)),
    },
    Err(err) => error_outcome(err),
  }
}

fn error_outcome<E: Serialize, P: Serialize>(err: CommandErrorResponseWrapper<E, P>) -> ToolOutcome {
  let value = serde_json::to_value(&err).unwrap_or(Value::Null);
  let message = err.error_message.clone().unwrap_or_else(|| "ArtCraft returned an error.".to_string());
  ToolOutcome::error(format!("{}\n\n{}", message, value))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn job_tokens_are_deduplicated_in_order() {
    let response = json!({ "inference_job_token": "jinf_1", "all_job_tokens": ["jinf_1", "jinf_2"] });
    assert_eq!(job_tokens(&response), vec!["jinf_1", "jinf_2"]);
  }

  #[test]
  fn missing_jobs_are_reported_as_not_found() {
    assert_eq!(summarize_job("jinf_x", None)["status"], "not_found");
  }
}
