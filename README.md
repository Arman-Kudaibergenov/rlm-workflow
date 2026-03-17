# RLM Workflow for Claude Code

RLM Workflow — это публичный workflow-слой вокруг `RLM-Toolkit` для сохранения контекста между сессиями Claude Code.

## Документация

- Русская документация: [README.ru.md](README.ru.md)
- Installation: [docs/en/installation.md](docs/en/installation.md)
- Workflow rituals: [docs/en/workflow.md](docs/en/workflow.md)
- Hooks: [docs/en/hooks.md](docs/en/hooks.md)
- LLM backends: [docs/en/llm-alternatives.md](docs/en/llm-alternatives.md)
- Multi-agent mode: [docs/en/multi-agent.md](docs/en/multi-agent.md)

## Что это дает

- сохранение рабочих фактов и решений между сессиями
- восстановление контекста после `/compact`
- единый memory layer для нескольких агентов
- project-scoped workflow поверх базового RLM

## Быстрый старт

### Docker

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

## Volumes: что обязательно, а что нет

Есть две разные задачи:

- обязательный volume для самой памяти RLM: `rlm-data:/data`
- опциональный bind mount проекта, если вы хотите file-based discovery внутри контейнера

Для обычной работы RLM как memory server достаточно только `rlm-data:/data`. Исходники проекта монтируются отдельно и только если вы хотите анализировать файловое дерево прямо из контейнера.

Пример:

```bash
docker run -d \
  --name rlm \
  -p 8200:8200 \
  -v rlm-data:/data \
  -v /path/to/project:/workspace:ro \
  -e RLM_PROJECT_ROOT=/workspace \
  ghcr.io/arman-kudaibergenov/rlm-workflow:latest
```

## Windows note

На Docker Desktop for Windows bind mount может ломаться на путях с пробелами или кириллицей. В таком случае сначала сделайте ASCII alias или junction и монтируйте уже его.

## Attribution

Этот проект построен поверх [RLM-Toolkit](https://github.com/DmitrL-dev/AISecurity/tree/main/rlm-toolkit) Дмитрия Лабинцева. Базовая memory architecture, иерархические факты и causal decision tracking принадлежат исходному проекту. Наш слой — это workflow, упаковка и практический операционный контур.

Подробности: [CREDITS.md](CREDITS.md), [CHANGELOG.md](CHANGELOG.md)
