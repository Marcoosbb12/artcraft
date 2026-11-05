MCP Server (Claude and other AI assistants)
===========================================

The ArtCraft desktop app runs a local [Model Context Protocol](https://modelcontextprotocol.io)
server. With it, Claude Code, Claude Desktop, and other MCP clients can generate images and videos
through ArtCraft. They use the account you're logged into in the app, and they spend the same
credits. Results show up in the app as usual.

## Requirements

- An ArtCraft build that includes the MCP server, running, and logged in.
- Node.js 18+ for the stdio bridge (only needed for Claude Desktop or other stdio-only clients).

## How it works

| Item       | Value                                                           |
|------------|-----------------------------------------------------------------|
| Endpoint   | `http://127.0.0.1:41950/mcp` (Streamable HTTP, stateless)       |
| Bind       | `127.0.0.1` only, so it can't be reached from other machines    |
| Auth       | `Authorization: Bearer <token>`                                 |
| Token file | `~/Artcraft/credentials/mcp_server.json` (`url`, `port`, `token`) |

The token is created on first start and reused after that. To get a new one, delete
`mcp_server.json` and restart the app. Requests from web pages (any `Origin` header) and requests
with a non-loopback `Host` header are refused, so websites can't spend your credits.

Environment variables:

| Variable                | Effect                                       |
|-------------------------|----------------------------------------------|
| `ARTCRAFT_MCP_PORT`     | Listen on another port (default `41950`)     |
| `ARTCRAFT_MCP_DISABLED` | `1` / `true` turns the server off            |
| `ARTCRAFT_MCP_CONFIG`   | Bridge only: path to `mcp_server.json`       |

## Tools

| Tool                      | What it does                                                        |
|---------------------------|---------------------------------------------------------------------|
| `artcraft_list_models`    | Image or video models, with their valid options                     |
| `artcraft_estimate_cost`  | Credit cost of a generation, without running it                     |
| `artcraft_generate_image` | Start an image generation (spends credits); returns job tokens      |
| `artcraft_generate_video` | Start a video generation (spends credits); returns job tokens       |
| `artcraft_get_jobs`       | Job status, and result URL + media token; `wait_seconds` up to 120  |
| `artcraft_get_credits`    | Credit balance                                                      |

Model-specific options that aren't listed in a tool's schema can go in `extra_options`. They are
passed to the ArtCraft API unchanged.

## Claude Code

Recommended: use the bridge. It reads the token file itself, so you never copy the token.

```bash
claude mcp add --scope user artcraft -- node /path/to/artcraft/script/artcraft/artcraft_mcp_bridge.mjs
```

Or connect over HTTP directly (replace `<token>` with the `token` from `mcp_server.json`):

```bash
claude mcp add --scope user --transport http artcraft http://127.0.0.1:41950/mcp --header "Authorization: Bearer <token>"
```

Check it with `claude mcp list`, or `/mcp` inside Claude Code.

## Claude Desktop

Open Settings → Developer → Edit Config and add this to `claude_desktop_config.json`, using the
absolute path to the bridge script:

```json
{
  "mcpServers": {
    "artcraft": {
      "command": "node",
      "args": ["/path/to/artcraft/script/artcraft/artcraft_mcp_bridge.mjs"]
    }
  }
}
```

On Windows, use a path like `C:\\Users\\you\\artcraft\\script\\artcraft\\artcraft_mcp_bridge.mjs`.
Restart Claude Desktop afterwards. If ArtCraft isn't open, the tools answer with a message
asking you to open it.

## Checking the server by hand

```bash
TOKEN=$(node -p "require(require('os').homedir() + '/Artcraft/credentials/mcp_server.json').token")
curl -s http://127.0.0.1:41950/mcp -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Code

- `crates/desktop/artcraft/src/core/mcp/`: protocol, tools, HTTP transport, and the executor
  that calls the same Tauri commands the UI uses
- `crates/desktop/artcraft/src/core/lifecycle/startup/tasks/spawn_mcp_server.rs`: startup
- `script/artcraft/artcraft_mcp_bridge.mjs`: stdio bridge
