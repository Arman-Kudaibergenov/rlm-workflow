# SDD: Fix GitHub Issues #3-#12

**Status:** DRAFT
**Date:** 2026-03-23
**Repo:** rlm-workflow (github.com/Arman-Kudaibergenov/rlm-workflow)

---

## 1. Problem

12 open issues from user Serg2000Mr. Mix of upstream rlm-toolkit bugs (patched in Docker) and our repo fixes.

## 2. Issue Map

### A. Upstream rlm-toolkit patches (applied in start_server.py)

| # | Title | Root Cause | Fix |
|---|-------|-----------|-----|
| #5/#4 | Embedding env vars ignored | `server.py:141` hardcodes `all-MiniLM-L6-v2`, ignores `RLM_EMBEDDING_MODEL` | Monkey-patch in `start_server.py`: read env vars, pass model to `EmbeddingService` and `HierarchicalMemoryStore.set_embedder()` after `create_server()` |
| #8 | `rlm_start_session(restore=true)` without session_id creates empty session | `manager.py:72`: generates random UUID when `session_id=None`, then `load_state()` finds nothing | Monkey-patch `MemoryBridgeManager.start_session()`: when `restore=True` and `session_id=None`, try loading `"default"` session first, else find most recent session from storage |
| #7 | Inconsistent default session behavior | Same root as #8 | Fixed by same patch |

### B. Our repo fixes

| # | Title | File | Fix |
|---|-------|------|-----|
| #10 | `$event` reserved PowerShell variable | `examples/hooks/context-monitor.ps1:21` | Rename `$event` → `$toolEvent` |
| #11 | 300s staleness threshold too small | `context-monitor.ps1:30` | Increase to 3600s, add env var `$env:RLM_CTX_STALE_SEC` |
| #12 | CLAUDE.md.example missing tools + H-MEM table | `examples/CLAUDE.md.example` | Add `rlm_get_facts_by_domain` to контекст, add `rlm_get_stale_facts`/`rlm_consolidate_facts` to суммаризируем, add H-MEM levels table |
| #9 | "контекст" as first word ruins chat history titles | `examples/CLAUDE.md.example` | Add note: start with task description, then "контекст" on second line |
| #6 | `root_path: "/data"` misleading | `docker/docker-compose.yml`, docs | Add comment in compose + note in README about bind mount for discover_project |
| #3 | Volume name mismatch docker run vs compose | `docker/docker-compose.yml` | Use `external: true` with named volume `rlm-data`, add migration note in README |

### C. Docker improvements

| Item | Fix |
|------|-----|
| Healthcheck uses `/sse` | Change to transport-aware check (streamable-http → `/mcp`, sse → `/sse`) |
| docker-compose missing embedding env vars | Add `RLM_EMBEDDING_MODEL` and `RLM_EMBEDDING_PROVIDER` to environment |
| entrypoint.sh doesn't log embedding config | Add echo for embedding model |

---

## 3. Implementation: start_server.py patches

### Embedding fix (#5/#4)

After `create_server()`, override the embedder:

```python
embedding_model = os.environ.get("RLM_EMBEDDING_MODEL", "")
if embedding_model:
    from sentence_transformers import SentenceTransformer
    embedder = SentenceTransformer(embedding_model)
    server.memory_bridge_v2_store.set_embedder(embedder)
    print(f"  Embedding: {embedding_model} (from RLM_EMBEDDING_MODEL)")
else:
    print(f"  Embedding: all-MiniLM-L6-v2 (default)")
```

### Session restore fix (#8/#7)

Monkey-patch `MemoryBridgeManager.start_session`:

```python
import types
from rlm_toolkit.memory_bridge.manager import MemoryBridgeManager

_original_start_session = MemoryBridgeManager.start_session

def _patched_start_session(self, session_id=None, restore=True):
    if restore and session_id is None:
        # Try "default" session first
        state = self.storage.load_state("default")
        if state:
            state.version += 1
            from datetime import datetime
            state.timestamp = datetime.now()
            self._current_state = state
            return state
        # Else try most recent session from storage
        if hasattr(self.storage, 'list_sessions'):
            sessions = self.storage.list_sessions()
            if sessions:
                latest = max(sessions, key=lambda s: s.get('timestamp', ''))
                state = self.storage.load_state(latest['session_id'])
                if state:
                    state.version += 1
                    state.timestamp = datetime.now()
                    self._current_state = state
                    return state
    return _original_start_session(self, session_id=session_id, restore=restore)

MemoryBridgeManager.start_session = _patched_start_session
```

---

## 4. Implementation Stages

### Stage 1: start_server.py patches (#5, #4, #8, #7)
- Embedding model override from env
- Session restore fallback to "default" / most recent

### Stage 2: context-monitor.ps1 fixes (#10, #11)
- Rename `$event` → `$toolEvent`
- Threshold 300 → configurable via `$env:RLM_CTX_STALE_SEC`, default 3600

### Stage 3: CLAUDE.md.example improvements (#12, #9)
- Add missing tools to rituals
- Add H-MEM levels table
- Add note about chat history titles

### Stage 4: Docker fixes (#6, #3, healthcheck, compose)
- docker-compose.yml: embedding env vars, volume notes
- Dockerfile: transport-aware healthcheck
- entrypoint.sh: log embedding config
- README notes for volume migration and discover_project

### Stage 5: Local Docker test
- Build image locally
- Test embedding override
- Test session restore
- Test healthcheck

### Stage 6: Publish
- Build + push to ghcr.io
- Close issues with commit references

---

## 5. Acceptance Criteria

1. [ ] `RLM_EMBEDDING_MODEL=paraphrase-multilingual-MiniLM-L12-v2` → server loads that model
2. [ ] `rlm_start_session(restore=true)` without session_id restores last session (not empty)
3. [ ] `$event` renamed in context-monitor.ps1
4. [ ] Staleness threshold configurable, default 3600s
5. [ ] CLAUDE.md.example includes rlm_get_facts_by_domain, rlm_get_stale_facts, rlm_consolidate_facts, H-MEM table
6. [ ] docker-compose.yml includes embedding env vars
7. [ ] All 10 issues addressable by these changes (some may need README notes only)
8. [ ] Docker image builds and passes healthcheck
9. [ ] Local test: embedding model, session restore, tool calls — all work

## 6. No-Go

- Breaking existing rlm-toolkit API
- Modifying installed rlm-toolkit package files (only monkey-patch in start_server.py)
- Publishing Docker image before all local tests pass
