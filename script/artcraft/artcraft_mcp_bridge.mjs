#!/usr/bin/env node
// Stdio <-> HTTP bridge for the ArtCraft MCP server.
//
// MCP clients that only launch local commands (e.g. Claude Desktop) run this
// script. It forwards each JSON-RPC message from stdin to the MCP server inside
// the running ArtCraft app and writes the replies to stdout. No dependencies;
// needs Node.js 18+.
//
// Connection details come from <ArtCraft data dir>/credentials/mcp_server.json,
// which the app writes on startup. Override the file with ARTCRAFT_MCP_CONFIG.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

const CONFIG_PATH =
  process.env.ARTCRAFT_MCP_CONFIG ??
  join(homedir(), "Artcraft", "credentials", "mcp_server.json");

const NOT_RUNNING_MESSAGE =
  "ArtCraft is not running, or its MCP server is off. Open the ArtCraft app, log in, and try again.";

function loadConfig() {
  // Re-read on every message so an app restart (or new token) is picked up.
  return JSON.parse(readFileSync(CONFIG_PATH, "utf8"));
}

function write(message) {
  process.stdout.write(JSON.stringify(message) + "\n");
}

function errorReply(id, message) {
  return { jsonrpc: "2.0", id, error: { code: -32000, message } };
}

async function forward(line) {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    write({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
    return;
  }

  const id = message?.id ?? null;
  const expectsReply = id !== null && typeof message?.method === "string";

  let response;
  try {
    const config = loadConfig();
    response = await fetch(config.url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        authorization: `Bearer ${config.token}`,
      },
      body: line,
    });
  } catch {
    if (expectsReply) write(errorReply(id, NOT_RUNNING_MESSAGE));
    return;
  }

  if (response.status === 202) return;

  const text = await response.text();
  if (!response.ok) {
    if (expectsReply) write(errorReply(id, `ArtCraft MCP server error (${response.status}): ${text}`));
    return;
  }

  const reply = JSON.parse(text);
  for (const item of Array.isArray(reply) ? reply : [reply]) write(item);
}

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", (line) => {
  if (line.trim()) {
    forward(line).catch((err) => process.stderr.write(`artcraft-mcp-bridge: ${err}\n`));
  }
});
