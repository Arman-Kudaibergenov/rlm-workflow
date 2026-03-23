# RLM Workflow для Claude Code

> Персистентная память ИИ между сессиями на базе RLM-Toolkit — кастомизированный рабочий процесс для Claude Code.

**[Read in English](README.md)**

---

## Проблема

Каждый раз когда контекст Claude заполняется — вы теряете всё. Архитектурные решения, незавершённые задачи, "почему так сделано" — всё исчезает при `/compact` или перезапуске. Следующая сессия начинается с нуля.

## Решение

RLM-Toolkit — внешний MCP-сервер, хранящий факты и решения персистентно. Claude обращается к памяти через MCP-инструменты. Три кодовые фразы управляют всем:

```
контекст          # начало сессии — восстановить где остановились
суммаризируем     # конец сессии — сохранить всё важное
новая задача      # старт задачи — инициализировать task_id, создать команду
```

## Быстрый старт

```bash
# Запустить RLM-сервер (FastEmbed, без GPU и API-ключей)
docker run -d --name rlm --restart unless-stopped -p 8200:8200 -v rlm-data:/data ghcr.io/arman-kudaibergenov/rlm-workflow:latest

# Или через Docker Compose
curl -O https://raw.githubusercontent.com/Arman-Kudaibergenov/rlm-workflow/master/docker/docker-compose.yml
docker compose up -d
```

## Volume: что обязательно, а что нет

Есть два разных сценария:

- обязательно для обычной работы RLM: volume вида `rlm-data:/data`
- опционально для доступа контейнера к файлам проекта: отдельный bind mount с проектом

Для самой памяти RLM исходники проекта не нужны. Стандартного `-v rlm-data:/data` достаточно для фактов, индексов и продолжения сессий.

Если нужен file-based discovery внутри контейнера, добавьте второй mount и укажите `RLM_PROJECT_ROOT`:

```bash
docker run -d --name rlm --restart unless-stopped -p 8200:8200 -v rlm-data:/data -v /path/to/project:/workspace:ro -e RLM_PROJECT_ROOT=/workspace ghcr.io/arman-kudaibergenov/rlm-workflow:latest
```

На Docker Desktop для Windows bind mount может ломаться на путях с пробелами и кириллицей. В таком случае лучше смонтировать ASCII-алиас или junction.

Добавить в `~/.claude/mcp.json`:
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

Скопировать шаблон CLAUDE.md в корень проекта (**обязательно** — без него ритуалы не работают):
```bash
curl -o CLAUDE.md https://raw.githubusercontent.com/Arman-Kudaibergenov/rlm-workflow/master/examples/CLAUDE.md.example
# Отредактируйте секции [YOUR ...] под свой проект
```

---

## Документация

| Документ | Описание |
|----------|----------|
| [Что такое RLM](docs/ru/что-такое-rlm.md) | Проблема, концепция, ключевые идеи |
| [Установка](docs/ru/установка.md) | Docker, pip, удалённый сервер |
| [Рабочий процесс](docs/ru/рабочий-процесс.md) | Три ритуала: контекст / суммаризируем / новая задача |
| [Хуки](docs/ru/хуки.md) | Автоматизация: pre-compact, context-monitor, auto-capture |
| [Варианты LLM](docs/ru/альтернативы-llm.md) | FastEmbed, OpenAI, Ollama — что выбрать |
| [Мульти-агент](docs/ru/мульти-агент.md) | Shared memory для команд агентов Claude |
| [Безопасность](docs/ru/безопасность.md) | Что хранится, как закрыть, TTL для секретов |
| [Сравнение](docs/ru/сравнение.md) | vs Mem0, Zep, MemGPT, EncrEor/rlm-claude |

---

## MCP-инструменты сервера

После подключения в Claude Code доступны:

| Инструмент | Назначение |
|-----------|-----------|
| `rlm_start_session(restore)` | Инициализация / восстановление сессии |
| `rlm_enterprise_context(query)` | Умная загрузка контекста проекта — **рекомендуется** |
| `rlm_add_hierarchical_fact(content, level, domain, ttl_days)` | Сохранить факт (L0–L3) |
| `rlm_record_causal_decision(decision, reasons, consequences, alternatives)` | Сохранить архитектурное решение с обоснованием |
| `rlm_search_facts(query, top_k, semantic_weight, keyword_weight, recency_weight)` | Гибридный поиск |
| `rlm_route_context(query, max_tokens)` | Семантическая маршрутизация — только релевантное |
| `rlm_sync_state()` | Зафиксировать состояние на диск |
| `rlm_discover_project(project_root)` | Авто-обнаружение типа проекта |

---

## Что кастомизировано относительно оригинала

Оригинальный RLM-Toolkit создан для обработки 10M+ токенов. Мы адаптировали его под ежедневную разработку с Claude Code:

| Добавлено нами | Описание |
|----------------|----------|
| Ритуалы сессии | `контекст` / `суммаризируем` / `новая задача` |
| `pre-compact.ps1` | Авто-сохранение перед `/compact` без участия пользователя |
| `context-monitor.ps1` | Мониторинг % контекста, авто-суммаризируем при 80% |
| `auto-capture.ps1` | Логирование Edit/Write/Bash в буфер для точного отчёта |
| `statusline.ps1` | Строка статуса с токенами, лимитами API, статусом MCP |
| PENDING-паттерн | Структурированные задачи с `[task_id]` и `[project:]` scoping |
| Мульти-агент | Протокол shared memory + checkpoint для команд агентов |
| `task_id` | Сквозной ключ задачи через все сессии для ретроспектив |

---

## Примеры

- [`examples/CLAUDE.md.example`](examples/CLAUDE.md.example) — шаблон CLAUDE.md для нового проекта
- [`examples/hooks/`](examples/hooks/) — реальные хуки PowerShell

---

## Авторство

Этот проект — кастомизация **RLM-Toolkit** авторства [Дмитрия Лабинцева (DmitrL-dev)](https://github.com/DmitrL-dev).

Мы взяли оригинальный процесс и адаптировали под свой рабочий процесс разработки ПО.
Базовая архитектура памяти, инструменты MCP и иерархические факты — работа оригинального автора.

### Статьи автора на Хабре
- [Полное руководство по обработке 10M+ токенов](https://habr.com/ru/articles/986280/)
- [Почему ваш LLM-агент забывает цель](https://habr.com/ru/articles/986836/)
- [RLM-Toolkit v1.2.1: Теоретические основы](https://habr.com/ru/articles/986702/)
- [RLM-Toolkit: Полная замена LangChain? FAQ часть 2](https://habr.com/ru/articles/987250/)

Полное авторство: [CREDITS.md](CREDITS.md)

## Лицензия

[Apache License 2.0](LICENSE) — как и оригинальный RLM-Toolkit.
