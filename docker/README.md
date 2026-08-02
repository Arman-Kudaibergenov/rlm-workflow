# RLM server — reproducible Docker image

Образ `rlm-server:20260802-2` воспроизводит живой прод-деплой (rlm-toolkit 2.3.1
+ `start_server.py` с патчами #5..#47: rlm_diff, freshness, card-эмбеддинги,
FTS5+RRF, реранкер с circuit breaker, loadout-профили, bi-temporal as_of,
revisor авто-актуальности). Тег `20260802` — версия до revisor (#46).

## Сборка

Контекст — корень репо:

```bash
docker build -f docker/Dockerfile.prod -t rlm-server:20260802-2 .
```

Собирался на отдельном build-хосте с docker (в LXC прода docker нет).
Готовые tar на build-хосте: `/root/rlm-server_20260802-2.tar` (текущий, 586 МБ) и
`/root/rlm-server_20260802.tar`; развёрнутый образ — 2.73 ГБ.

## Запуск

```bash
docker run -d --name rlm \
  -p 8250:8250 \
  -v rlm-data:/data \
  rlm-server:20260802-2
```

Требования к volume: `/data/.rlm/memory/memory_bridge_v2.db` — **мигрированная**
БД (card-эмбеддинги + bi-temporal). Миграции одноразовые, уже применены к
продовой БД; в чистый volume сначала скопируй продовую БД, либо прогони
`/app/migrate_card_embeddings.py` и `/app/migrate_bitemporal.py` внутри
контейнера (потребуется доступ к Ollama <ollama-host>:11434).

Env (уже в образе, можно переопределить):
- `RLM_EMBEDDING_MODEL=all-MiniLM-L6-v2` — модель запечена в образ; смена
  модели = пересборка + переэмбеддинг БД, иначе поиск сломается (#37).
- `RERANK_ENABLED=true`, `RERANK_URL=http://<reranker-host>:8400` — реранкер с
  circuit breaker (120с), при недоступности порядок RRF, `reranked:false`.
- `RLM_TOOLS` — `all` или список; по умолчанию 18 инструментов.

## Revisor (#47) — авто-актуальность

Фоновая задача внутри сервера (не sidecar). Три уровня: TTL-правила по
доменам → ping эндпоинтов из content (TCP 2с / HTTP HEAD; живой эндпоинт
спасает факт) → опциональный LLM-вердикт (только deep-проходы).

Env:
- `REVISOR_MODE=report` (default; только отчёт) | `auto` (инвалидирует через
  закрытие validity window, лимит 20 за проход).
- `REVISOR_ENABLED=true` (default) — расписание: ежедневно 04:17 лок.
  (уровни 1-2), воскресенье 04:17 — deep (+LLM).
- `REVISOR_LLM_URL` (напр. `http://<ollama-host>:11434/v1`) +
  `REVISOR_LLM_MODEL` (default `qwen3:8b`). Без URL уровень 3 пропускается.
- Ручной запуск: инструмент `rlm_revisor_run(deep=false, mode="")`.

Конфиг `/data/revisor.yaml` (в volume; при отсутствии — дефолты кода):

```yaml
default_ttl_days: 90
ttl_days:
  work_current: 7
  tasks: 30
  project_status: 30
  infrastructure: 90
  deploy: 30
  deployment: 30
never: [pitfalls, decisions, security, licensing, reference, causal, revisor]
```

Каждый проход пишет факт-отчёт (level 1, domain `revisor`): режим,
кандидаты N, инвалидировано M (id+причина), спасено пингом K.

ВНИМАНИЕ: `REVISOR_MODE=auto` на большой базе начнёт инвалидировать
TTL-просроченные факты (до 20 за проход, ежедневно). Восстановление:
`UPDATE hierarchical_facts SET is_stale=0, valid_until=NULL WHERE ...`.

## Smoke (прогнан 2026-08-02 на build-хосте)

```bash
docker run -d --name rlm-smoke -p 18250:8250 -v /tmp/rlm-smoke-data:/data rlm-server:20260802-2
# initialize (protocolVersion 2025-03-26) -> mcp-session-id
# notifications/initialized -> tools/list = 17 -> route_context ok
docker stop rlm-smoke && docker rm rlm-smoke
```

## Миграция с systemd на контейнер (будущая, НЕ выполнена)

1. `systemctl stop rlm` на прод-хосте.
2. Скопировать `/data/.rlm` (или хотя бы `memory/memory_bridge_v2.db`) в volume.
3. `docker load < rlm-server_20260802-2.tar`, `docker run` как выше, порт 8250.
4. Переключить клиентов, неделю держать systemd-юнит выключенным (не удалять).
5. Откат: stop контейнер, `systemctl start rlm` — БД совместима (та же схема,
   soft-delete только добавляет строки).
