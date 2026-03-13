"""Thin wrapper to launch rlm-toolkit MCP server with configurable transport and bind address."""

import argparse
import os


def main():
    parser = argparse.ArgumentParser(description="RLM-Toolkit MCP server launcher")
    parser.add_argument("--host", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=8200)
    parser.add_argument("--transport", default="sse", choices=["stdio", "sse", "streamable-http"])
    args = parser.parse_args()

    # Ensure RLM_PROJECT_ROOT is set before import (ContextManager reads it at init)
    os.environ.setdefault("RLM_PROJECT_ROOT", "/data")

    from rlm_toolkit.mcp.server import create_server

    server = create_server()
    server.mcp.settings.host = args.host
    server.mcp.settings.port = args.port
    server.mcp.run(transport=args.transport)


if __name__ == "__main__":
    main()
