"""Docker healthcheck for RLM MCP server. Supports SSE and streamable-http transports."""

import json
import os
import urllib.request

transport = os.environ.get("RLM_TRANSPORT", "sse")
base = "http://localhost:8200"

if transport == "streamable-http":
    # Streamable-http requires JSON-RPC POST with MCP initialize
    payload = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "healthcheck", "version": "1.0"},
        },
    }).encode()
    req = urllib.request.Request(
        f"{base}/mcp",
        data=payload,
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        },
    )
    urllib.request.urlopen(req, timeout=5)
else:
    # SSE transport — simple GET
    urllib.request.urlopen(f"{base}/sse", timeout=5)
