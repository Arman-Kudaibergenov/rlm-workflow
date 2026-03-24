# SDD: Fix GitHub Issues #13–#18

**Project:** rlm-workflow
**Date:** 2026-03-24
**Type:** Bug fixes (6 issues)

## Issues Summary

| # | Severity | Title | Root Cause |
|---|----------|-------|------------|
| 13 | CRITICAL | float32 not JSON serializable | SentenceTransformer.encode() returns numpy float32; rlm-toolkit serializes to JSON without conversion |
| 14 | HIGH | entrypoint overwrites RLM_PROJECT_ROOT | Unconditional `export RLM_PROJECT_ROOT="$DATA_DIR"` in entrypoint.sh |
| 15 | MEDIUM | Embedding provider contract mismatch | .env.example advertises openai/ollama but code only uses SentenceTransformer |
| 16 | MEDIUM | Multiple embedding model loads | Upstream creates default embedder, then we patch with override — double load |
| 17 | LOW | FileWatcher started log misleading | Upstream logs "started" regardless of start_file_watcher() return value |
| 18 | MEDIUM | Session restore fallback to latest | restore=true + session_id=None falls back to ANY latest session |

## Fixes

### #13: float32 JSON serialization (CRITICAL)

**Root cause:** `SentenceTransformer.encode()` returns `numpy.ndarray` with `dtype=float32`. When rlm-toolkit stores the fact, it serializes embeddings to JSON. `json.dumps()` can't handle `numpy.float32`.

**Fix:** Wrap the `SentenceTransformer` embedder with a proxy that converts output to Python floats before returning. In `start_server.py`, after `embedder = SentenceTransformer(model_name)`:

```python
class Float32SafeEmbedder:
    """Wraps SentenceTransformer to convert float32 → Python float for JSON safety."""

    def __init__(self, model: SentenceTransformer):
        self._model = model

    def encode(self, sentences, **kwargs):
        result = self._model.encode(sentences, **kwargs)
        # Convert numpy float32 → Python float
        if hasattr(result, 'tolist'):
            return result.tolist()
        return result

    def get_sentence_embedding_dimension(self):
        return self._model.get_sentence_embedding_dimension()

    def __getattr__(self, name):
        return getattr(self._model, name)
```

Then: `server.memory_bridge_v2_store.set_embedder(Float32SafeEmbedder(embedder))`

### #14: RLM_PROJECT_ROOT overwrite

**Fix:** In `entrypoint.sh` line 20, change unconditional export to conditional:

```bash
export RLM_PROJECT_ROOT="${RLM_PROJECT_ROOT:-$DATA_DIR}"
```

### #15: Embedding provider contract

**Fix:** Honest approach — document that only `sentence-transformers`-compatible models are supported. Remove `RLM_EMBEDDING_PROVIDER` from `.env.example` (it's unused). Update comments to clarify.

### #16: Multiple embedding model loads

**Fix:** Set `RLM_EMBEDDING_MODEL` env var BEFORE `create_server()` so rlm-toolkit's default initialization can read it. But since rlm-toolkit 2.3.1 hardcodes `all-MiniLM-L6-v2`, the real fix is: suppress upstream's default model initialization by patching `RLMServer._init_store()` to skip embedder creation if `RLM_EMBEDDING_MODEL` is set.

Actually, simpler: live with double load but log clearly. The cost is ~2s extra at startup, happens once. Not worth complex patching. Add a comment explaining why.

**Revised fix:** Just log clearly that default model will be replaced, and ensure _patch_embedding() runs quickly after create_server().

### #17: FileWatcher started log

**Fix:** After `create_server()`, patch the logger to suppress misleading messages. Or: add a note to docs that `watchdog` is not installed in the Docker image and FileWatcher messages can be ignored.

Actually, simpler: install watchdog in Dockerfile, or add logging filter. Since this is upstream behavior, install watchdog:

```dockerfile
RUN pip install --no-cache-dir "rlm-toolkit[all]==2.3.1" watchdog
```

**Revised:** Don't add watchdog (unnecessary dep for container). Instead, after server creation, check if file watcher is actually running and log corrected status.

**Final fix:** Add after `_patch_embedding(server)`:

```python
# Suppress misleading "FileWatcher started" — watchdog not installed in container
import logging
logging.getLogger("rlm_toolkit.memory_bridge.v2.ttl").setLevel(logging.ERROR)
```

### #18: Session restore fallback isolation

**Fix:** Remove the "latest session" fallback. Only restore "default" session. If no "default" exists, create a new empty one and log a warning.

```python
def _patched(self, session_id=None, restore=True):
    if restore and session_id is None:
        # Only restore named "default" session — no fallback to latest (isolation)
        state = self.storage.load_state("default")
        if state:
            state.version += 1
            state.timestamp = datetime.now()
            self._current_state = state
            print(f"  Session restored: 'default' (v{state.version})")
            return state
        print("  No 'default' session found — creating new session")
    return _original(self, session_id=session_id, restore=restore)
```

## Implementation Stages

### Stage 1: Critical fix — float32 serialization (#13)
- Add `Float32SafeEmbedder` wrapper class to `start_server.py`
- Wrap SentenceTransformer instance before passing to `set_embedder()`
- **Verify:** Docker test: `rlm_add_hierarchical_fact` succeeds with paraphrase-multilingual model

### Stage 2: Entrypoint + session restore (#14, #18)
- Fix RLM_PROJECT_ROOT conditional export in entrypoint.sh
- Remove latest-session fallback, keep only "default" session restore
- **Verify:** Docker test: container respects external RLM_PROJECT_ROOT; session restore uses "default" only

### Stage 3: Docs and logging (#15, #16, #17)
- Remove RLM_EMBEDDING_PROVIDER from .env.example, clarify sentence-transformers only
- Add log note about double model load in start_server.py
- Suppress misleading FileWatcher log via logging level
- **Verify:** Container logs clean, no misleading messages

## Acceptance Criteria

1. `rlm_add_hierarchical_fact` works with `paraphrase-multilingual-MiniLM-L12-v2`
2. `rlm_search_facts` returns facts saved with multilingual model
3. Container with `-e RLM_PROJECT_ROOT=/workspace -v /path:/workspace:ro` uses `/workspace`
4. Container without RLM_PROJECT_ROOT uses `/data` (default)
5. `restore=true, session_id=None` restores "default" session only, not latest
6. If no "default" session exists, new session created (no silent fallback)
7. .env.example accurately reflects supported configuration
8. No "FileWatcher started" when watchdog not installed
9. All existing functionality (search, delete, consolidate) continues working

## No-Go Criteria

- Breaking existing stored facts/embeddings (dimension mismatch)
- Removing any functional capability
- Adding external dependencies not already in rlm-toolkit[all]
