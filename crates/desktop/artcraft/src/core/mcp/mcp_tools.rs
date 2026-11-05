//! MCP tool definitions and argument parsing.
//!
//! Pure, Tauri-free code: tool JSON schemas, and the conversion of tool arguments
//! into calls the executor runs.

use serde_json::{json, Map, Value};

const MAX_WAIT_SECONDS: u64 = 120;
const MAX_JOB_TOKENS: usize = 20;

const TOOL_LIST_MODELS: &str = "artcraft_list_models";
const TOOL_ESTIMATE_COST: &str = "artcraft_estimate_cost";
const TOOL_GENERATE_IMAGE: &str = "artcraft_generate_image";
const TOOL_GENERATE_VIDEO: &str = "artcraft_generate_video";
const TOOL_GET_JOBS: &str = "artcraft_get_jobs";
const TOOL_GET_CREDITS: &str = "artcraft_get_credits";

/// Generation fields the image tool accepts directly. Anything else goes in `extra_options`.
const IMAGE_FIELDS: &[&str] = &[
  "model", "prompt", "aspect_ratio", "resolution", "quality", "batch_size", "image_media_tokens",
];

/// Generation fields the video tool accepts directly. Anything else goes in `extra_options`.
const VIDEO_FIELDS: &[&str] = &[
  "model", "prompt", "negative_prompt", "aspect_ratio", "resolution", "duration_seconds",
  "generate_audio", "start_frame_image_media_token", "end_frame_image_media_token",
  "reference_image_media_tokens",
];

/// A parsed, validated tool call.
#[derive(Debug, PartialEq)]
pub enum McpToolCall {
  ListModels { kind: MediaKind },
  EstimateCost { kind: MediaKind, fields: Map<String, Value> },
  Generate { kind: MediaKind, fields: Map<String, Value> },
  GetJobs { job_tokens: Vec<String>, wait_seconds: u64 },
  GetCredits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
  Image,
  Video,
}

impl MediaKind {
  pub fn from_str(value: &str) -> Option<Self> {
    match value {
      "image" => Some(Self::Image),
      "video" => Some(Self::Video),
      _ => None,
    }
  }
}

/// The `tools/list` result.
pub fn tool_definitions() -> Value {
  json!([
    {
      "name": TOOL_LIST_MODELS,
      "description": "List the image or video models available in ArtCraft, with their supported aspect ratios, resolutions, durations and other options. Call this before generating to pick a valid model id and options.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "kind": { "type": "string", "enum": ["image", "video"] }
        },
        "required": ["kind"]
      },
      "annotations": { "readOnlyHint": true }
    },
    {
      "name": TOOL_ESTIMATE_COST,
      "description": "Estimate the ArtCraft credit cost of a generation without running it. Takes the same arguments as artcraft_generate_image / artcraft_generate_video plus `kind`.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "kind": { "type": "string", "enum": ["image", "video"] },
          "model": { "type": "string" },
          "prompt": { "type": "string" },
          "extra_options": { "type": "object" }
        },
        "required": ["kind", "model"],
        "additionalProperties": true
      },
      "annotations": { "readOnlyHint": true }
    },
    {
      "name": TOOL_GENERATE_IMAGE,
      "description": "Generate images with ArtCraft. Spends the user's ArtCraft credits. Returns job tokens; then call artcraft_get_jobs (with wait_seconds) to get the image URLs. The result also appears in the ArtCraft app.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "model": { "type": "string", "description": "Model id from artcraft_list_models (kind=image)." },
          "prompt": { "type": "string" },
          "aspect_ratio": { "type": "string", "description": "e.g. square, wide_sixteen_by_nine, tall_nine_by_sixteen." },
          "resolution": { "type": "string", "description": "e.g. one_k, two_k, four_k." },
          "quality": { "type": "string" },
          "batch_size": { "type": "integer", "minimum": 1, "maximum": 4 },
          "image_media_tokens": { "type": "array", "items": { "type": "string" }, "description": "Reference images as ArtCraft media file tokens (mf_...)." },
          "extra_options": { "type": "object", "description": "Any other model-specific option, passed through unchanged." }
        },
        "required": ["model", "prompt"]
      }
    },
    {
      "name": TOOL_GENERATE_VIDEO,
      "description": "Generate a video with ArtCraft. Spends the user's ArtCraft credits. Returns job tokens; then call artcraft_get_jobs (with wait_seconds) to get the video URL. The result also appears in the ArtCraft app.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "model": { "type": "string", "description": "Model id from artcraft_list_models (kind=video)." },
          "prompt": { "type": "string" },
          "negative_prompt": { "type": "string" },
          "aspect_ratio": { "type": "string", "description": "e.g. wide_sixteen_by_nine, tall_nine_by_sixteen." },
          "resolution": { "type": "string", "description": "e.g. seven_twenty_p, ten_eighty_p." },
          "duration_seconds": { "type": "integer", "minimum": 1 },
          "generate_audio": { "type": "boolean" },
          "start_frame_image_media_token": { "type": "string", "description": "Media file token (mf_...) of the first frame, for image-to-video." },
          "end_frame_image_media_token": { "type": "string" },
          "reference_image_media_tokens": { "type": "array", "items": { "type": "string" } },
          "extra_options": { "type": "object", "description": "Any other model-specific option, passed through unchanged." }
        },
        "required": ["model", "prompt"]
      }
    },
    {
      "name": TOOL_GET_JOBS,
      "description": "Get the status of ArtCraft generation jobs and, once complete, the result URL and media file token. Set wait_seconds to wait for completion (up to 120).",
      "inputSchema": {
        "type": "object",
        "properties": {
          "job_tokens": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
          "wait_seconds": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT_SECONDS }
        },
        "required": ["job_tokens"]
      },
      "annotations": { "readOnlyHint": true }
    },
    {
      "name": TOOL_GET_CREDITS,
      "description": "Get the signed-in user's ArtCraft credit balance.",
      "inputSchema": { "type": "object", "properties": {} },
      "annotations": { "readOnlyHint": true }
    }
  ])
}

/// Parse a `tools/call` request into a validated call, or a message for the model.
pub fn parse_tool_call(name: &str, arguments: Option<&Value>) -> Result<McpToolCall, String> {
  let empty = Map::new();
  let args = match arguments {
    None | Some(Value::Null) => &empty,
    Some(Value::Object(args)) => args,
    Some(_) => return Err("Tool arguments must be an object.".into()),
  };

  match name {
    TOOL_LIST_MODELS => Ok(McpToolCall::ListModels { kind: required_kind(args)? }),
    TOOL_ESTIMATE_COST => {
      let kind = required_kind(args)?;
      Ok(McpToolCall::EstimateCost { kind, fields: generation_fields(kind, args)? })
    }
    TOOL_GENERATE_IMAGE => {
      let fields = generation_fields(MediaKind::Image, args)?;
      require_prompt(&fields)?;
      Ok(McpToolCall::Generate { kind: MediaKind::Image, fields })
    }
    TOOL_GENERATE_VIDEO => {
      let fields = generation_fields(MediaKind::Video, args)?;
      require_prompt(&fields)?;
      Ok(McpToolCall::Generate { kind: MediaKind::Video, fields })
    }
    TOOL_GET_JOBS => parse_get_jobs(args),
    TOOL_GET_CREDITS => Ok(McpToolCall::GetCredits),
    other => Err(format!("Unknown tool: {}", other)),
  }
}

/// Build the generation request body: the known fields, plus `extra_options`
/// (which never overrides a known field).
fn generation_fields(kind: MediaKind, args: &Map<String, Value>) -> Result<Map<String, Value>, String> {
  let model = args.get("model").and_then(Value::as_str).map(str::trim).unwrap_or_default();
  if model.is_empty() {
    return Err("`model` is required. Call artcraft_list_models to see the model ids.".into());
  }

  let allowed = match kind {
    MediaKind::Image => IMAGE_FIELDS,
    MediaKind::Video => VIDEO_FIELDS,
  };

  let mut fields = Map::new();
  for key in allowed {
    if let Some(value) = args.get(*key).filter(|value| !value.is_null()) {
      fields.insert((*key).to_owned(), value.clone());
    }
  }

  match args.get("extra_options") {
    None | Some(Value::Null) => {}
    Some(Value::Object(extra)) => {
      for (key, value) in extra {
        if is_local_only_field(key) {
          continue;
        }
        fields.entry(key.clone()).or_insert_with(|| value.clone());
      }
    }
    Some(_) => return Err("`extra_options` must be an object.".into()),
  }

  Ok(fields)
}

fn parse_get_jobs(args: &Map<String, Value>) -> Result<McpToolCall, String> {
  let job_tokens: Vec<String> = match args.get("job_tokens") {
    Some(Value::Array(values)) => values.iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect(),
    Some(Value::String(token)) if !token.trim().is_empty() => vec![token.trim().to_owned()],
    _ => Vec::new(),
  };

  if job_tokens.is_empty() {
    return Err("`job_tokens` must list at least one job token.".into());
  }
  if job_tokens.len() > MAX_JOB_TOKENS {
    return Err(format!("At most {} job tokens per call.", MAX_JOB_TOKENS));
  }

  let wait_seconds = args.get("wait_seconds")
      .and_then(Value::as_u64)
      .unwrap_or(0)
      .min(MAX_WAIT_SECONDS);

  Ok(McpToolCall::GetJobs { job_tokens, wait_seconds })
}

fn required_kind(args: &Map<String, Value>) -> Result<MediaKind, String> {
  args.get("kind")
      .and_then(Value::as_str)
      .and_then(MediaKind::from_str)
      .ok_or_else(|| "`kind` must be \"image\" or \"video\".".to_string())
}

fn require_prompt(fields: &Map<String, Value>) -> Result<(), String> {
  let has_prompt = fields.get("prompt")
      .and_then(Value::as_str)
      .is_some_and(|prompt| !prompt.trim().is_empty());
  if has_prompt { Ok(()) } else { Err("`prompt` is required.".into()) }
}

/// Desktop-local metadata that must not be settable through MCP.
fn is_local_only_field(key: &str) -> bool {
  matches!(key, "provider" | "frontend_caller" | "frontend_subscriber_id" | "frontend_subscriber_payload")
}

#[cfg(test)]
mod tests {
  use super::*;

  mod generate_tests {
    use super::*;

    #[test]
    fn image_keeps_known_fields_and_merges_extra_options() {
      let call = parse_tool_call(TOOL_GENERATE_IMAGE, Some(&json!({
        "model": "nano_banana",
        "prompt": "a cat",
        "aspect_ratio": "tall_nine_by_sixteen",
        "unknown_top_level": 1,
        "extra_options": { "seed": 7, "prompt": "ignored", "provider": "fal" }
      }))).unwrap();

      let expected = json!({
        "model": "nano_banana", "prompt": "a cat", "aspect_ratio": "tall_nine_by_sixteen", "seed": 7
      });
      assert_eq!(call, McpToolCall::Generate { kind: MediaKind::Image, fields: expected.as_object().unwrap().clone() });
    }

    #[test]
    fn video_accepts_video_fields() {
      let call = parse_tool_call(TOOL_GENERATE_VIDEO, Some(&json!({
        "model": "kling_2p1_pro", "prompt": "waves", "duration_seconds": 5, "generate_audio": true
      }))).unwrap();
      match call {
        McpToolCall::Generate { kind: MediaKind::Video, fields } => {
          assert_eq!(fields["duration_seconds"], 5);
          assert_eq!(fields["generate_audio"], true);
        }
        other => panic!("unexpected call: {:?}", other),
      }
    }

    #[test]
    fn missing_model_is_rejected() {
      let err = parse_tool_call(TOOL_GENERATE_IMAGE, Some(&json!({ "prompt": "a cat" }))).unwrap_err();
      assert!(err.contains("model"));
    }

    #[test]
    fn missing_prompt_is_rejected() {
      let err = parse_tool_call(TOOL_GENERATE_VIDEO, Some(&json!({ "model": "x", "prompt": "  " }))).unwrap_err();
      assert!(err.contains("prompt"));
    }

    #[test]
    fn estimate_does_not_require_prompt() {
      let call = parse_tool_call(TOOL_ESTIMATE_COST, Some(&json!({ "kind": "video", "model": "x" }))).unwrap();
      assert!(matches!(call, McpToolCall::EstimateCost { kind: MediaKind::Video, .. }));
    }
  }

  mod get_jobs_tests {
    use super::*;

    #[test]
    fn wait_is_capped() {
      let call = parse_tool_call(TOOL_GET_JOBS, Some(&json!({ "job_tokens": ["jinf_1"], "wait_seconds": 9999 }))).unwrap();
      assert_eq!(call, McpToolCall::GetJobs { job_tokens: vec!["jinf_1".into()], wait_seconds: MAX_WAIT_SECONDS });
    }

    #[test]
    fn empty_tokens_are_rejected() {
      assert!(parse_tool_call(TOOL_GET_JOBS, Some(&json!({ "job_tokens": [] }))).is_err());
    }
  }

  #[test]
  fn unknown_tool_is_rejected() {
    assert!(parse_tool_call("nope", None).is_err());
  }

  #[test]
  fn every_definition_parses_by_name() {
    let definitions = tool_definitions();
    for definition in definitions.as_array().unwrap() {
      let name = definition["name"].as_str().unwrap();
      let result = parse_tool_call(name, None);
      assert!(!matches!(&result, Err(message) if message.starts_with("Unknown tool")), "{}", name);
    }
  }
}
