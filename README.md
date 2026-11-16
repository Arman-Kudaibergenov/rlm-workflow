# RLM — иерархический memory MCP-сервер + workflow-слой

**v3.0.0 — основная реализация на Rust (`rlm-rs`), в проде с 2026-11-11.**
Python-референс (RLM-Toolkit + `docker/start_server.py`) сохранён в `docker/`: docker-путь,
эталон для регрессий и откат. Обе реализации разделяют один контракт: 18 MCP-инструментов,
схема 2.0.0 (`memory_bridge_v2.db`), MCP streamable-HTTP.

## Две реализации

| | Rust — основная | Python — референс |
|---|---|---|
| Код | `src/`, `Cargo.toml` | `docker/` |
| Сборка | `cargo build --release` | образ `ghcr.io/arman-kudaibergenov/rlm-workflow:latest` |
| Эмбеддинги | `all-MiniLM-L6-v2` на [candle](https://github.com/huggingface/candle), in-process | sentence-transformers / Ollama / OpenAI |
| RSS | ~110 МБ | ~514 МБ |
| Данные | та же `memory_bridge_v2.db`, без миграции | та же |

## Быстрый старт — Rust

```bash
cargo build --release
./target/release/rlm-rs --transport streamable-http --host 0.0.0.0 --port 8250
```

Environment: `RLM_DATA_DIR` (default `/data/.rlm`), `RLM_EMBEDDING_MODEL`
(default `all-MiniLM-L6-v2`), `RLM_EMBEDDING_PROVIDER` (local/ollama/openai),
`RERANK_ENABLED` / `RERANK_URL`, `REVISOR_MODE`, `RLM_STALE_WEIGHT`, `HF_HUB_OFFLINE`.
Модель грузится из локального HF-кэша; смена модели запускает автоматическую переиндексацию.

## Быстрый старт — Python / Docker (референс-путь)

```bash
docker run -d \
  --name rlm \
  -p 8200:8200 \
  -v rlm-data:/data \
  ghcr.io/arman-kudaibergenov/rlm-workflow:latest
```

### Docker Compose

```bash
curl -O https://raw.githubusercontent.com/Arman-Kudaibergenov/rlm-workflow/master/docker/docker-compose.yml
cp docker/.env.example .env
docker compose up -d
```

### Volumes

Обязательный volume для самой памяти RLM: `rlm-data:/data`. Опциональный bind mount проекта —
только если нужен file-based discovery внутри контейнера:

```bash
docker run -d \
  --name rlm \
  -p 8200:8200 \
  -v rlm-data:/data \
  -v /path/to/project:/workspace:ro \
  -e RLM_PROJECT_ROOT=/workspace \
  ghcr.io/arman-kudaibergenov/rlm-workflow:latest
```

### Windows note

На Docker Desktop for Windows bind mount может ломаться на путях с пробелами или кириллицей.
В таком случае сначала сделайте ASCII alias или junction и монтируйте уже его.

## Верификация Rust ↔ Python

- Инструменты: 18/18, имена и схемы — 0 расхождений (`compare_tools.py`).
- Регрессии: идентичные pass/fail на обеих реализациях (17/17, 8/9, 20/21, 11/11, 16/16;
  2 падения — артефакты продакшн-данных, одинаковые на обеих сторонах).
- Эмбеддинги: cosine с sentence-transformers = 1.000000 (`parity_check.py`).
- Качество маршрутизации: hit@5 0.48 → 0.74 после включения видимости stale-фактов
  (`RLM_STALE_WEIGHT`, default 1.0); archived по-прежнему исключены.
- Память: RSS 514 → 107 МБ на проде.

Подробности: [VERIFY.md](VERIFY.md). Продакшн-бенч приватен; публичный — нейтральный шаблон
`tests/bench_quality.json` (ожидания регенерируются под вашу базу, см. docstring `bench_quality.py`).

## Workflow-слой

- сохранение рабочих фактов и решений между сессиями
- восстановление контекста после `/compact`
- единый memory layer для нескольких агентов
- project-scoped workflow поверх базового RLM

Документация:

- Русская документация: [README.ru.md](README.ru.md)
- Installation: [docs/en/installation.md](docs/en/installation.md)
- Workflow rituals: [docs/en/workflow.md](docs/en/workflow.md)
- Hooks: [docs/en/hooks.md](docs/en/hooks.md)
- LLM backends: [docs/en/llm-alternatives.md](docs/en/llm-alternatives.md)
- Multi-agent mode: [docs/en/multi-agent.md](docs/en/multi-agent.md)

## Attribution

Этот проект построен поверх [RLM-Toolkit](https://github.com/DmitrL-dev/AISecurity/tree/main/rlm-toolkit)
Дмитрия Лабинцева. Базовая memory architecture, иерархические факты и causal decision tracking
принадлежат исходному проекту. Наш слой — workflow, упаковка и практический операционный контур,
а с v3.0.0 — Rust-реализация сервера.

Подробности: [CREDITS.md](CREDITS.md), [CHANGELOG.md](CHANGELOG.md).
Лицензия: [LICENSE](LICENSE), [NOTICE](NOTICE).
