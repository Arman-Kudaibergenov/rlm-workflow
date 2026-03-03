# Installation Guide

## Prerequisites

- Docker and Docker Compose installed
- Claude Code CLI installed
- 2GB RAM minimum for the RLM container

## Option 1: Docker (Recommended)

### Pull and run

```bash
docker run -d \
  --name rlm \
  -p 8200:8200 \
  -v rlm-data:/data \
  ghcr.io/admin/rlm:latest
```

### Docker Compose

```bash
# Download compose file
curl -O https://raw.githubusercontent.com/admin/rlm/main/docker/docker-compose.yml
curl -O https://raw.githubusercontent.com/admin/rlm/main/docker/.env.example
cp .env.example .env

# Edit .env if needed (default: fastembed, no API key required)
# nano .env

# Start
docker compose up -d

# Verify
curl http://localhost:8200/health
```

## Option 2: pip (without Docker)

```bash
pip install rlm-toolkit
rlm-server --host 0.0.0.0 --port 8200 --data-dir ./data
```

## Configure Claude Code

Add to `~/.claude/mcp.json`:

```json
{
  "mcpServers": {
    "rlm-toolkit": {
      "type": "http",
      "url": "http://localhost:8200/mcp"
    }
  }
}
```

Restart Claude Code — run `claude mcp list` to verify the server is connected.

## Configure CLAUDE.md workflow

Copy the example CLAUDE.md template to your project:

```bash
curl -O https://raw.githubusercontent.com/admin/rlm/main/examples/CLAUDE.md.example
# Rename and customize for your project
```

See [workflow.md](workflow.md) for the full ritual patterns.

## Install Hooks (for automation)

See [hooks.md](hooks.md) for pre-compact and context-monitor automation.
These hooks make RLM save automatically without any manual `суммаризируем` commands.

## Remote Server Setup

If running RLM on a remote server (recommended for teams):

```bash
# On the server
docker compose up -d

# On your local machine — add to ~/.claude/mcp.json
{
  "mcpServers": {
    "rlm-toolkit": {
      "type": "http",
      "url": "http://YOUR_SERVER_IP:8200/mcp"
    }
  }
}
```

> **Security note**: RLM stores all your conversation context. Run it on a trusted server
> behind a firewall, or add authentication (see [security.md](security.md)).
