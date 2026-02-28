# Credits & Attribution

## Original Project

**RLM-Toolkit** is developed by **Dmitry Labintcev** ([@DmitrL-dev](https://github.com/DmitrL-dev)).

- GitHub: https://github.com/DmitrL-dev/AISecurity/tree/main/rlm-toolkit
- PyPI: `pip install rlm-toolkit`

### Articles by the original author (Habr / Хабр)

| Article | URL |
|---------|-----|
| Полное руководство по обработке 10M+ токенов | https://habr.com/ru/articles/986280/ |
| Почему ваш LLM-агент забывает цель | https://habr.com/ru/articles/986836/ |
| RLM-Toolkit v1.2.1: Теоретические основы и оригинальные разработки | https://habr.com/ru/articles/986702/ |
| RLM-Toolkit: Полная замена LangChain? FAQ часть 2 | https://habr.com/ru/articles/987250/ |

---

## This Project

This repository documents a **customized workflow** built on top of RLM-Toolkit. We took the original process and adapted it for a software development workflow with Claude Code.

**What comes from the original:**
- Core memory architecture (hierarchical facts, causal decisions)
- `rlm_add_hierarchical_fact`, `rlm_record_causal_decision`, `rlm_enterprise_context` MCP tools
- Semantic search infrastructure
- Session management primitives

**What we added:**
- Pre-compact hook automation (auto-save before context wipe)
- Context-monitor hook (auto-save at configurable context % threshold)
- PENDING task tracking pattern with project-scope filtering
- Multi-agent team memory coordination protocol
- Autocapture buffer for tool-use logging
- CLAUDE.md ritual patterns (`суммаризируем` / `контекст` / `новая задача`)
- Docker packaging for easy deployment

---

## Related Projects

Other RLM implementations that inspired the ecosystem:

- **EncrEor/rlm-claude** (MIT) — https://github.com/EncrEor/rlm-claude
  Infinite memory solution for Claude Code with pre-compact hooks

- **delonsp/rlm-mcp-server** (MIT) — https://github.com/delonsp/rlm-mcp-server
  Original RLM MCP server implementation

- **DEV.to article** — https://dev.to/encreor/how-i-gave-claude-code-infinite-memory-using-mits-rlm-paper-2hhk
  "How I Gave Claude Code Infinite Memory (Using MIT's RLM Paper)"

---

## License

Original RLM-Toolkit: **Apache License 2.0**
This repository: **Apache License 2.0** (same, as required)

See [LICENSE](LICENSE) and [NOTICE](NOTICE) for full terms.
