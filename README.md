# RLM Workflow for Claude Code

> Persistent AI memory across sessions using RLM-Toolkit — customized workflow for Claude Code.

**Основная документация на русском: [README.ru.md](README.ru.md)**

---

## The Problem

Every time Claude's context window fills up and you run `/compact` (or it happens automatically), you lose everything: decisions made, architecture discussed, tasks in progress. Next session — you start from scratch.

**RLM solves this** by giving Claude a persistent external memory store that survives context clears and `/compact` events.

## What Is RLM

[RLM-Toolkit](https://github.com/DmitrL-dev/AISecurity/tree/main/rlm-toolkit) is an open-source memory layer for LLM agents, developed by [Dmitry Labintcev (DmitrL-dev)](https://github.com/DmitrL-dev). It implements hierarchical memory with causal decision tracking, semantic search, and enterprise-grade context management.

This repository documents our **customized workflow** built on top of RLM-Toolkit for daily software development with Claude Code.

### Original author's articles on Habr (Russian)
- [Полное руководство по обработке 10M+ токенов](https://habr.com/ru/articles/986280/)
- [Почему ваш LLM-агент забывает цель](https://habr.com/ru/articles/986836/)
- [RLM-Toolkit v1.2.1: Теоретические основы](https://habr.com/ru/articles/986702/)
- [RLM-Toolkit: Полная замена LangChain? FAQ часть 2](https://habr.com/ru/articles/987250/)

## Quick Start (Docker)

```bash
# Pull and run the RLM server
docker run -d \
  --name rlm \
  -p 8200:8200 \
  -v rlm-data:/data \
  ghcr.io/arman-kudaibergenov/rlm-workflow:latest

# Add to Claude Code MCP config (~/.claude/mcp.json)
# See docs/en/installation.md for full configuration
```

Or with Docker Compose:
```bash
curl -O https://raw.githubusercontent.com/Arman-Kudaibergenov/rlm-workflow/main/docker/docker-compose.yml
cp docker/.env.example .env
# Edit .env with your settings
docker compose up -d
```

## What's Customized

This workflow extends the original RLM-Toolkit with:

| Feature | Original | Our Workflow |
|---------|----------|--------------|
| Session continuity | Manual | Automated via pre-compact hook |
| Context monitoring | None | Auto-save at 65% context usage |
| Multi-agent memory | None | Shared RLM across agent teams |
| Task tracking | None | PENDING facts with project scoping |
| Autocapture | None | Automatic tool-use logging |

See [CHANGELOG.md](CHANGELOG.md) for full list of customizations.

## LLM Backend Options

RLM-Toolkit can use various LLM backends for semantic search and embeddings:

| Option | Hardware needed | Setup complexity |
|--------|----------------|------------------|
| OpenAI API | None (cloud) | Low — just API key |
| Ollama + nomic-embed | CPU only | Medium |
| Ollama + qwen3:8b | GPU recommended | Medium |
| FastEmbed (built-in) | CPU only | Zero |

See [docs/en/llm-alternatives.md](docs/en/llm-alternatives.md) for details.

## Documentation

- [Installation guide](docs/en/installation.md)
- [Workflow rituals](docs/en/workflow.md) — the `summarize` / `context` / `new task` patterns
- [Hooks setup](docs/en/hooks.md) — pre-compact and context-monitor automation
- [LLM alternatives](docs/en/llm-alternatives.md)
- [Multi-agent workflow](docs/en/multi-agent.md)

## Credits & Attribution

This project is a customization of **RLM-Toolkit** by [Dmitry Labintcev](https://github.com/DmitrL-dev).

We took the original process and adapted it for our software development workflow. The core memory architecture, hierarchical facts, and causal decision tracking are from the original work.

Full attribution: [CREDITS.md](CREDITS.md)

## License

This project is distributed under the [Apache License 2.0](LICENSE), same as the original RLM-Toolkit.

Per Apache 2.0 requirements: the original copyright notice and NOTICE file are preserved. Our modifications are documented in [CHANGELOG.md](CHANGELOG.md).
