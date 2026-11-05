//! JSON-RPC message handling for the MCP server (transport independent).

use std::future::Future;

use serde_json::{json, Value};

use crate::core::mcp::mcp_tools::{parse_tool_call, tool_definitions, McpToolCall};
use crate::version::ARTCRAFT_VERSION;

/// Protocol revisions this server speaks, newest first.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const SERVER_NAME: &str = "artcraft";

const SERVER_INSTRUCTIONS: &str = "ArtCraft generates images and videos with the user's ArtCraft account and credits. \
Typical flow: artcraft_list_models to choose a model and valid options, optionally artcraft_estimate_cost, \
then artcraft_generate_image or artcraft_generate_video, then artcraft_get_jobs with wait_seconds until the job \
is complete_success and share the result URL. Generations spend credits, so do not repeat a generate call to retry.";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What the transport should send back.
#[derive(Debug, PartialEq)]
pub enum McpReply {
  /// A JSON-RPC response (or batch of responses).
  Json(Value),
  /// Only notifications or responses were received: HTTP 202 with no body.
  Accepted,
}

/// The outcome of running a tool, reported to the model as text.
#[derive(Debug, PartialEq)]
pub struct ToolOutcome {
  pub text: String,
  pub is_error: bool,
}

/// Runs validated tool calls. Implemented against the Tauri app, and by fakes in tests.
pub trait McpToolExecutor {
  fn execute(&self, call: McpToolCall) -> impl Future<Output = ToolOutcome> + Send;
}

impl ToolOutcome {
  pub fn ok(value: &Value) -> Self {
    Self {
      text: serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
      is_error: false,
    }
  }

  pub fn error(message: impl Into<String>) -> Self {
    Self { text: message.into(), is_error: true }
  }
}

/// Handle one HTTP POST body: a single message or a batch.
pub async fn handle_body<E: McpToolExecutor>(body: &[u8], executor: &E) -> McpReply {
  let parsed: Value = match serde_json::from_slice(body) {
    Ok(value) => value,
    Err(err) => return McpReply::Json(error_response(Value::Null, PARSE_ERROR, &format!("Parse error: {}", err))),
  };

  match parsed {
    Value::Array(messages) => {
      let mut responses = Vec::new();
      for message in messages {
        if let Some(response) = handle_message(message, executor).await {
          responses.push(response);
        }
      }
      if responses.is_empty() { McpReply::Accepted } else { McpReply::Json(Value::Array(responses)) }
    }
    message => match handle_message(message, executor).await {
      Some(response) => McpReply::Json(response),
      None => McpReply::Accepted,
    },
  }
}

/// Returns `None` for notifications and client responses, which get no reply.
async fn handle_message<E: McpToolExecutor>(message: Value, executor: &E) -> Option<Value> {
  let Value::Object(message) = message else {
    return Some(error_response(Value::Null, INVALID_REQUEST, "Invalid request"));
  };

  let id = message.get("id").cloned();
  let Some(method) = message.get("method").and_then(Value::as_str) else {
    // A response to something we sent (we send nothing), or junk.
    return id.map(|id| error_response(id, INVALID_REQUEST, "Invalid request"));
  };
  let id = id?; // Notifications (e.g. notifications/initialized) need no reply.
  let params = message.get("params");

  let response = match method {
    "initialize" => success_response(id, initialize_result(params)),
    "ping" => success_response(id, json!({})),
    "tools/list" => success_response(id, json!({ "tools": tool_definitions() })),
    "tools/call" => {
      let Some(name) = params.and_then(|params| params.get("name")).and_then(Value::as_str) else {
        return Some(error_response(id, INVALID_PARAMS, "tools/call needs a tool `name`"));
      };
      let arguments = params.and_then(|params| params.get("arguments"));
      let outcome = match parse_tool_call(name, arguments) {
        Ok(call) => executor.execute(call).await,
        Err(message) => ToolOutcome::error(message),
      };
      success_response(id, tool_result(outcome))
    }
    "resources/list" => success_response(id, json!({ "resources": [] })),
    "prompts/list" => success_response(id, json!({ "prompts": [] })),
    other => error_response(id, METHOD_NOT_FOUND, &format!("Method not found: {}", other)),
  };

  Some(response)
}

fn initialize_result(params: Option<&Value>) -> Value {
  let requested = params
      .and_then(|params| params.get("protocolVersion"))
      .and_then(Value::as_str);

  let version = requested
      .filter(|requested| SUPPORTED_PROTOCOL_VERSIONS.contains(requested))
      .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);

  json!({
    "protocolVersion": version,
    "capabilities": { "tools": { "listChanged": false } },
    "serverInfo": { "name": SERVER_NAME, "title": "ArtCraft", "version": ARTCRAFT_VERSION },
    "instructions": SERVER_INSTRUCTIONS,
  })
}

fn tool_result(outcome: ToolOutcome) -> Value {
  json!({
    "content": [{ "type": "text", "text": outcome.text }],
    "isError": outcome.is_error,
  })
}

fn success_response(id: Value, result: Value) -> Value {
  json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
  json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Mutex;

  mod lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn initialize_echoes_a_supported_version() {
      let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).await;
      assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
      assert_eq!(reply["result"]["serverInfo"]["name"], "artcraft");
    }

    #[tokio::test]
    async fn initialize_falls_back_to_latest_version() {
      let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#).await;
      assert_eq!(reply["result"]["protocolVersion"], SUPPORTED_PROTOCOL_VERSIONS[0]);
    }

    #[tokio::test]
    async fn notifications_are_accepted_without_a_body() {
      let reply = handle_body(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, &FakeExecutor::default()).await;
      assert_eq!(reply, McpReply::Accepted);
    }

    #[tokio::test]
    async fn unknown_methods_are_errors() {
      let reply = call(r#"{"jsonrpc":"2.0","id":"a","method":"nope"}"#).await;
      assert_eq!(reply["error"]["code"], METHOD_NOT_FOUND);
      assert_eq!(reply["id"], "a");
    }

    #[tokio::test]
    async fn malformed_json_is_a_parse_error() {
      let reply = call("{not json").await;
      assert_eq!(reply["error"]["code"], PARSE_ERROR);
    }
  }

  mod tool_tests {
    use super::*;

    #[tokio::test]
    async fn tools_list_returns_definitions() {
      let reply = call(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).await;
      assert!(reply["result"]["tools"].as_array().unwrap().len() >= 5);
    }

    #[tokio::test]
    async fn valid_calls_reach_the_executor() {
      let executor = FakeExecutor::default();
      let body = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"artcraft_get_credits","arguments":{}}}"#;
      let McpReply::Json(reply) = handle_body(body.as_bytes(), &executor).await else { panic!("expected json") };
      assert_eq!(reply["result"]["isError"], false);
      assert_eq!(executor.calls.lock().unwrap().as_slice(), &[McpToolCall::GetCredits]);
    }

    #[tokio::test]
    async fn invalid_arguments_never_reach_the_executor() {
      let executor = FakeExecutor::default();
      let body = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"artcraft_generate_image","arguments":{"prompt":"x"}}}"#;
      let McpReply::Json(reply) = handle_body(body.as_bytes(), &executor).await else { panic!("expected json") };
      assert_eq!(reply["result"]["isError"], true);
      assert!(executor.calls.lock().unwrap().is_empty());
    }
  }

  #[derive(Default)]
  struct FakeExecutor {
    calls: Mutex<Vec<McpToolCall>>,
  }

  impl McpToolExecutor for FakeExecutor {
    fn execute(&self, call: McpToolCall) -> impl Future<Output = ToolOutcome> + Send {
      self.calls.lock().unwrap().push(call);
      async { ToolOutcome::ok(&json!({ "ok": true })) }
    }
  }

  async fn call(body: &str) -> Value {
    match handle_body(body.as_bytes(), &FakeExecutor::default()).await {
      McpReply::Json(value) => value,
      McpReply::Accepted => panic!("expected a JSON reply"),
    }
  }
}
