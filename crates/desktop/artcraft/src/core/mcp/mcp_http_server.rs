//! MCP "Streamable HTTP" transport, bound to 127.0.0.1 only.
//!
//! Stateless: every POST carries one JSON-RPC message (or batch) and gets a JSON
//! reply. There is no server-to-client stream, so GET is answered with 405, as the
//! MCP spec allows.
//!
//! Generations spend credits, so every request must carry the bearer token from
//! `mcp_server.json`. Requests from web pages (any `Origin` header) and with a
//! foreign `Host` header (DNS rebinding) are refused before the token is checked.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};

use bytes::Bytes;
use errors::AnyhowResult;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderValue, ALLOW, AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{info, warn};
use tokio::net::TcpListener;

use crate::core::mcp::mcp_protocol::{handle_body, McpReply, McpToolExecutor};
use crate::core::mcp::mcp_server_config::McpServerConfig;

const MCP_PATH: &str = "/mcp";
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Accept connections until the app exits.
pub async fn run_mcp_http_server<E>(config: McpServerConfig, executor: E) -> AnyhowResult<()>
where
  E: McpToolExecutor + Clone + Send + Sync + 'static,
{
  let address = SocketAddr::from((Ipv4Addr::LOCALHOST, config.port));
  let listener = TcpListener::bind(address).await?;
  serve(listener, config, executor).await
}

async fn serve<E>(listener: TcpListener, config: McpServerConfig, executor: E) -> AnyhowResult<()>
where
  E: McpToolExecutor + Clone + Send + Sync + 'static,
{
  info!("MCP server listening on {}", config.url);

  loop {
    let (stream, _) = match listener.accept().await {
      Ok(connection) => connection,
      Err(err) => {
        warn!("MCP server accept failed: {:?}", err);
        continue;
      }
    };

    let config = config.clone();
    let executor = executor.clone();

    tokio::spawn(async move {
      let service = service_fn(move |request| {
        let config = config.clone();
        let executor = executor.clone();
        async move { Ok::<_, Infallible>(handle_request(request, &config, &executor).await) }
      });

      if let Err(err) = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await {
        warn!("MCP connection error: {:?}", err);
      }
    });
  }
}

async fn handle_request<E: McpToolExecutor>(
  request: Request<Incoming>,
  config: &McpServerConfig,
  executor: &E,
) -> Response<Full<Bytes>> {
  if let Err(rejection) = check_request(request.method(), request.uri().path(), request.headers(), config) {
    return rejection.into_response();
  }

  let body = match Limited::new(request.into_body(), MAX_BODY_BYTES).collect().await {
    Ok(body) => body.to_bytes(),
    Err(_) => return Rejection::new(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large or unreadable.").into_response(),
  };

  match handle_body(&body, executor).await {
    McpReply::Accepted => empty_response(StatusCode::ACCEPTED),
    McpReply::Json(value) => {
      let mut response = Response::new(Full::new(Bytes::from(value.to_string())));
      response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
      response
    }
  }
}

/// Why a request was refused before reaching the protocol layer.
#[derive(Debug, PartialEq)]
struct Rejection {
  status: StatusCode,
  message: &'static str,
}

impl Rejection {
  fn new(status: StatusCode, message: &'static str) -> Self {
    Self { status, message }
  }

  fn into_response(self) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(self.message)));
    *response.status_mut() = self.status;
    if self.status == StatusCode::METHOD_NOT_ALLOWED {
      response.headers_mut().insert(ALLOW, HeaderValue::from_static("POST"));
    }
    response
  }
}

fn check_request(method: &Method, path: &str, headers: &HeaderMap, config: &McpServerConfig) -> Result<(), Rejection> {
  if path != MCP_PATH {
    return Err(Rejection::new(StatusCode::NOT_FOUND, "Not found. The MCP endpoint is /mcp."));
  }
  if headers.contains_key(ORIGIN) {
    return Err(Rejection::new(StatusCode::FORBIDDEN, "Browser requests are not allowed."));
  }
  if !is_loopback_host(headers.get(HOST), config.port) {
    return Err(Rejection::new(StatusCode::FORBIDDEN, "Invalid Host header."));
  }
  if !has_valid_token(headers.get(AUTHORIZATION), &config.token) {
    return Err(Rejection::new(StatusCode::UNAUTHORIZED, "Missing or invalid ArtCraft MCP token."));
  }
  if method != Method::POST {
    return Err(Rejection::new(StatusCode::METHOD_NOT_ALLOWED, "Only POST is supported."));
  }
  Ok(())
}

fn is_loopback_host(host: Option<&HeaderValue>, port: u16) -> bool {
  let Some(host) = host.and_then(|host| host.to_str().ok()) else {
    return false;
  };
  ["127.0.0.1", "localhost"].iter().any(|name| {
    host.eq_ignore_ascii_case(name) || host.eq_ignore_ascii_case(&format!("{}:{}", name, port))
  })
}

fn has_valid_token(header: Option<&HeaderValue>, token: &str) -> bool {
  let Some(provided) = header
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.strip_prefix("Bearer "))
      .map(str::trim) else {
    return false;
  };
  constant_time_eq(provided.as_bytes(), token.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
  if left.len() != right.len() {
    return false;
  }
  left.iter().zip(right).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

fn empty_response(status: StatusCode) -> Response<Full<Bytes>> {
  let mut response = Response::new(Full::new(Bytes::new()));
  *response.status_mut() = status;
  response
}

#[cfg(test)]
mod tests {
  use super::*;

  const PORT: u16 = 41950;
  const TOKEN: &str = "0123456789abcdef0123456789abcdef";

  #[test]
  fn valid_request_is_allowed() {
    assert_eq!(check(Method::POST, MCP_PATH, &headers(&[("host", "127.0.0.1:41950"), ("authorization", "Bearer 0123456789abcdef0123456789abcdef")])), Ok(()));
  }

  #[test]
  fn localhost_host_is_allowed() {
    assert_eq!(check(Method::POST, MCP_PATH, &headers(&[("host", "localhost:41950"), ("authorization", "Bearer 0123456789abcdef0123456789abcdef")])), Ok(()));
  }

  mod rejection_tests {
    use super::*;

    #[test]
    fn missing_token_is_unauthorized() {
      assert_eq!(status(Method::POST, MCP_PATH, &headers(&[("host", "127.0.0.1:41950")])), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn wrong_token_is_unauthorized() {
      assert_eq!(status(Method::POST, MCP_PATH, &headers(&[("host", "127.0.0.1:41950"), ("authorization", "Bearer nope")])), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn browser_origin_is_forbidden_even_with_token() {
      let headers = headers(&[("host", "127.0.0.1:41950"), ("origin", "https://evil.example"), ("authorization", "Bearer 0123456789abcdef0123456789abcdef")]);
      assert_eq!(status(Method::POST, MCP_PATH, &headers), StatusCode::FORBIDDEN);
    }

    #[test]
    fn rebound_host_is_forbidden() {
      let headers = headers(&[("host", "evil.example:41950"), ("authorization", "Bearer 0123456789abcdef0123456789abcdef")]);
      assert_eq!(status(Method::POST, MCP_PATH, &headers), StatusCode::FORBIDDEN);
    }

    #[test]
    fn other_paths_are_not_found() {
      assert_eq!(status(Method::POST, "/", &headers(&[])), StatusCode::NOT_FOUND);
    }

    #[test]
    fn get_is_method_not_allowed() {
      let headers = headers(&[("host", "127.0.0.1:41950"), ("authorization", "Bearer 0123456789abcdef0123456789abcdef")]);
      assert_eq!(status(Method::GET, MCP_PATH, &headers), StatusCode::METHOD_NOT_ALLOWED);
    }
  }

  mod end_to_end_tests {
    use super::*;
    use crate::core::mcp::mcp_protocol::ToolOutcome;
    use crate::core::mcp::mcp_tools::McpToolCall;
    use serde_json::{json, Value};
    use std::future::Future;

    #[tokio::test]
    async fn tool_call_round_trips_over_http() {
      let url = start_server().await;
      let response = reqwest::Client::new()
          .post(&url)
          .bearer_auth(TOKEN)
          .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"artcraft_get_credits"}}))
          .send()
          .await
          .unwrap();
      assert_eq!(response.status(), reqwest::StatusCode::OK);
      let body: Value = response.json().await.unwrap();
      assert_eq!(body["result"]["content"][0]["text"], "GetCredits");
    }

    #[tokio::test]
    async fn notification_gets_202_and_bad_token_gets_401() {
      let url = start_server().await;
      let client = reqwest::Client::new();
      let accepted = client.post(&url).bearer_auth(TOKEN)
          .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
          .send().await.unwrap();
      assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);

      let refused = client.post(&url).bearer_auth("wrong")
          .json(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
          .send().await.unwrap();
      assert_eq!(refused.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    #[derive(Clone)]
    struct EchoExecutor;

    impl McpToolExecutor for EchoExecutor {
      fn execute(&self, call: McpToolCall) -> impl Future<Output = ToolOutcome> + Send {
        async move { ToolOutcome { text: format!("{:?}", call), is_error: false } }
      }
    }

    async fn start_server() -> String {
      let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
      let port = listener.local_addr().unwrap().port();
      let config = McpServerConfig::new(port, TOKEN.to_string());
      let url = config.url.clone();
      tokio::spawn(serve(listener, config, EchoExecutor));
      url
    }
  }

  fn check(method: Method, path: &str, headers: &HeaderMap) -> Result<(), Rejection> {
    check_request(&method, path, headers, &McpServerConfig::new(PORT, TOKEN.to_string()))
  }

  fn status(method: Method, path: &str, headers: &HeaderMap) -> StatusCode {
    check(method, path, headers).unwrap_err().status
  }

  fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
      map.insert(*name, HeaderValue::from_static(value));
    }
    map
  }
}
