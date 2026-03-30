"""Thin wrapper to launch rlm-toolkit MCP server with configurable transport and bind address.

Applies monkey-patches for:
- #13: Float32SafeEmbedder wrapper for numpy→Python float conversion
- #5/#4: Embedding model override from RLM_EMBEDDING_MODEL env var
- #8/#7/#18: Session restore — "default" session only, no fallback to latest
- #16: Single embedder instance reused across store and router
- #17: Suppress misleading FileWatcher log from upstream
- #19: search_facts — ensure session before hybrid_search
- #20: discover_project — normalize Windows paths to container paths
- #22: enterprise_context — log causal failures at WARNING, not DEBUG
- #23: project overview — suppress noisy fingerprint, improve Unknown project
- #24: double embedding load — prevent upstream default model init
"""

import argparse
import logging
import os


class Float32SafeEmbedder:
    """Wraps embedders to convert numpy float32 → float64 for JSON safety.

    Fixes GitHub issue #13: rlm-toolkit serializes embeddings to JSON, but numpy.float32
    is not JSON-serializable. This wrapper intercepts encode() output and converts to float64
    (JSON-serializable) while keeping numpy array structure (upstream may call .tolist()).
    """

    def __init__(self, model):
        self._model = model

    def encode(self, sentences, **kwargs):
        import numpy as np

        result = self._model.encode(sentences, **kwargs)
        if isinstance(result, np.ndarray):
            return result.astype(np.float64)
        # Handle edge case: list of numpy arrays
        if isinstance(result, list):
            return [r.astype(np.float64) if isinstance(r, np.ndarray) else r for r in result]
        return result

    def get_sentence_embedding_dimension(self):
        return self._model.get_sentence_embedding_dimension()

    def __getattr__(self, name):
        return getattr(self._model, name)


def _patch_session_restore():
    """Patch MemoryBridgeManager.start_session to use "default" as canonical session name.

    Fixes GitHub issues #8, #7, #18, #21: without this patch, restore=True with session_id=None
    either generates a random UUID (empty session) or falls back to latest session (isolation risk).

    Key insight (#21 fix): when session_id=None, we pass "default" to upstream so that
    sync_state saves under "default" — making future restore=True find the session.
    """
    from datetime import datetime

    from rlm_toolkit.memory_bridge.manager import MemoryBridgeManager

    _original = MemoryBridgeManager.start_session

    def _patched(self, session_id=None, restore=True):
        # When explicit session_id is provided, delegate to upstream unchanged (BLOCKER fix)
        if session_id is not None:
            return _original(self, session_id=session_id, restore=restore)

        # When no session_id, use "default" as canonical name.
        # This ensures sync_state saves under "default" and restore finds it.
        if restore:
            # Only restore named "default" session — no fallback to latest (isolation, #18)
            state = self.storage.load_state("default")
            if state:
                # Bump version on restore — MCP tool checks version > 1 for restored flag
                state.version += 1
                state.timestamp = datetime.now()
                self._current_state = state
                print(f"  Session restored: 'default' (v{state.version})")
                return state
            print("  No 'default' session found — creating new session")

        # Create new session under "default", continue version sequence to avoid UNIQUE constraint
        result = _original(self, session_id="default", restore=False)
        if result.version == 1:
            existing = self.storage.load_state("default")
            if existing and existing.version >= result.version:
                result.version = existing.version + 1
                self._current_state = result
        return result

    MemoryBridgeManager.start_session = _patched


class OllamaEmbedder:
    """Embedding via Ollama HTTP API. Supports models like nomic-embed-text:latest."""

    def __init__(self, model_name: str, base_url: str):
        self._model_name = model_name
        self._base_url = base_url.rstrip("/")
        self._dim = None

    def encode(self, sentences, **kwargs):
        import json
        import urllib.request

        import numpy as np

        if isinstance(sentences, str):
            sentences = [sentences]

        url = f"{self._base_url}/api/embed"
        payload = json.dumps({"model": self._model_name, "input": sentences}).encode()
        req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.loads(resp.read())

        embeddings = data.get("embeddings", [])
        if not self._dim and embeddings:
            self._dim = len(embeddings[0])
        # Return numpy arrays — upstream code calls .tolist() on results
        if len(embeddings) > 1:
            return np.array(embeddings, dtype=np.float32)
        return np.array(embeddings[0], dtype=np.float32) if embeddings else np.array([])

    def get_sentence_embedding_dimension(self):
        if not self._dim:
            # Probe with a test sentence
            self.encode("dimension probe")
        return self._dim


class OpenAIEmbedder:
    """Embedding via OpenAI-compatible API.

    Works with: OpenAI, LM Studio, LocalAI, vLLM, text-generation-inference.
    Set OPENAI_API_BASE for non-OpenAI endpoints (e.g. http://localhost:1234/v1).
    """

    # Static dimension lookup — avoids paid API call for dimension probe
    _KNOWN_DIMS = {
        "text-embedding-3-small": 1536,
        "text-embedding-3-large": 3072,
        "text-embedding-ada-002": 1536,
    }

    def __init__(self, model_name: str, api_key: str, base_url: str = ""):
        self._model_name = model_name
        self._api_key = api_key
        self._base_url = base_url.rstrip("/") if base_url else "https://api.openai.com/v1"
        self._dim = self._KNOWN_DIMS.get(model_name)

    def encode(self, sentences, **kwargs):
        import json
        import urllib.request

        import numpy as np

        if isinstance(sentences, str):
            sentences = [sentences]

        url = f"{self._base_url}/embeddings"
        payload = json.dumps({"model": self._model_name, "input": sentences}).encode()
        req = urllib.request.Request(url, data=payload, headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self._api_key}",
        })
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.loads(resp.read())

        # Sort by index — OpenAI may return items out of order in batch calls
        items = sorted(data["data"], key=lambda x: x["index"])
        embeddings = [item["embedding"] for item in items]
        if not self._dim and embeddings:
            self._dim = len(embeddings[0])
        if len(embeddings) > 1:
            return np.array(embeddings, dtype=np.float32)
        return np.array(embeddings[0], dtype=np.float32) if embeddings else np.array([])

    def get_sentence_embedding_dimension(self):
        if not self._dim:
            self.encode("dimension probe")
        return self._dim


def _create_embedder(model_name: str):
    """Create embedder based on RLM_EMBEDDING_PROVIDER env var.

    Supports:
    - "ollama": uses Ollama HTTP API (requires OLLAMA_BASE_URL)
    - "openai": uses OpenAI API (requires OPENAI_API_KEY)
    - default: uses SentenceTransformer (HuggingFace models)

    All providers are wrapped in Float32SafeEmbedder for JSON safety (#13) —
    upstream serializes embeddings to JSON, and numpy float32 is not JSON-serializable.
    """
    provider = os.environ.get("RLM_EMBEDDING_PROVIDER", "").lower()

    if provider == "ollama":
        base_url = os.environ.get("OLLAMA_BASE_URL", "http://localhost:11434")
        raw_embedder = OllamaEmbedder(model_name, base_url)
        dim = raw_embedder.get_sentence_embedding_dimension()
        # Wrap for JSON safety (#13) — Ollama returns numpy float32
        embedder = Float32SafeEmbedder(raw_embedder)
        print(f"  Embedding: {model_name} via Ollama at {base_url} (dim={dim})")
        return embedder, dim

    if provider == "openai":
        api_key = os.environ.get("OPENAI_API_KEY", "")
        base_url = os.environ.get("OPENAI_API_BASE", "")
        if not api_key and not base_url:
            raise ValueError("RLM_EMBEDDING_PROVIDER=openai but OPENAI_API_KEY not set")
        effective_model = model_name or "text-embedding-3-small"
        raw_embedder = OpenAIEmbedder(effective_model, api_key, base_url)
        dim = raw_embedder.get_sentence_embedding_dimension()
        # Wrap for JSON safety (#13) — OpenAI returns numpy float32
        embedder = Float32SafeEmbedder(raw_embedder)
        target = base_url or "OpenAI API"
        print(f"  Embedding: {effective_model} via {target} (dim={dim})")
        return embedder, dim

    # Default: SentenceTransformer (HuggingFace)
    from sentence_transformers import SentenceTransformer

    raw_embedder = SentenceTransformer(model_name)
    dim = raw_embedder.get_sentence_embedding_dimension()
    # Wrap for JSON safety (#13) — convert numpy float32 → Python float
    embedder = Float32SafeEmbedder(raw_embedder)
    print(f"  Embedding: {model_name} (dim={dim}, from RLM_EMBEDDING_MODEL)")
    return embedder, dim


def _patch_embedding(server):
    """Override embedding model from RLM_EMBEDDING_MODEL env var.

    Fixes GitHub issues #5, #4, #13, #16:
    - Wraps SentenceTransformer in Float32SafeEmbedder for JSON-safe output (#13)
    - Supports Ollama via HTTP API (RLM_EMBEDDING_PROVIDER=ollama)
    - Single embedder instance reused for both store and router (#16)
    """
    model_name = os.environ.get("RLM_EMBEDDING_MODEL", "")
    if not model_name:
        print("  Embedding: upstream default (no RLM_EMBEDDING_MODEL set)")
        return

    try:
        embedder, dim = _create_embedder(model_name)

        # Patch store embedder — single instance (#16)
        server.memory_bridge_v2_store.set_embedder(embedder)

        # Patch router to reuse same embedder (#16) — avoid loading a third model instance
        components = getattr(server, "memory_bridge_v2_components", {})
        router = components.get("router")
        if router and hasattr(router, "embedding_service"):
            router.embedding_service._model = embedder
            router.embedding_service._model_name = model_name

        # Lower similarity threshold for non-MiniLM models (different similarity scales)
        # MiniLM: cosine similarity ~0.5-0.9 for related texts
        # qwen3/Ollama: cosine similarity ~0.2-0.6 for related texts
        provider = os.environ.get("RLM_EMBEDDING_PROVIDER", "").lower()
        if provider in ("ollama", "openai"):
            from rlm_toolkit.memory_bridge.v2.router import SemanticRouter

            # Patch existing and future instances: lower threshold for non-MiniLM models
            _original_init = SemanticRouter.__init__

            def _patched_init(self, *args, **kwargs):
                _original_init(self, *args, **kwargs)
                self.similarity_threshold = 0.15

            SemanticRouter.__init__ = _patched_init

            # Also find and patch existing router instance via gc
            import gc
            for obj in gc.get_objects():
                if isinstance(obj, SemanticRouter):
                    obj.similarity_threshold = 0.15
                    print(f"  Router instance patched: threshold → 0.15")
            print("  Router class patched: threshold → 0.15 (external embedding model)")

        # Class-level patch: override EmbeddingService.model property to always use our embedder.
        # This catches ALL EmbeddingService instances (router creates its own during tool registration).
        from rlm_toolkit.memory_bridge.v2.embeddings import EmbeddingService

        EmbeddingService._patched_model = embedder
        EmbeddingService._patched_model_name = model_name

        _original_model_getter = EmbeddingService.model.fget

        @property
        def _patched_model_prop(self):
            if hasattr(EmbeddingService, "_patched_model") and EmbeddingService._patched_model is not None:
                self._model = EmbeddingService._patched_model
                self._model_name = EmbeddingService._patched_model_name
                return self._model
            return _original_model_getter(self)

        EmbeddingService.model = _patched_model_prop

    except Exception as e:
        import sys

        print(f"  ERROR: Embedding override failed for '{model_name}': {e}")
        print(f"  ERROR: RLM_EMBEDDING_MODEL={model_name} failed", file=sys.stderr)
        strict = os.environ.get("RLM_EMBEDDING_STRICT", "").lower() in ("true", "1", "yes")
        if strict:
            print(f"  FATAL: RLM_EMBEDDING_STRICT=true — refusing to start with wrong model")
            sys.exit(1)
        print(f"  WARNING: Falling back to upstream default embedding")


def _suppress_misleading_logs():
    """Suppress misleading 'FileWatcher started' from upstream when watchdog not installed.

    Fixes GitHub issue #17: upstream logs success regardless of start_file_watcher() return value.
    """
    import logging

    # Target ALL upstream loggers that may emit the misleading message
    for logger_name in ("rlm_mcp", "rlm_toolkit.mcp.server", "rlm_toolkit.mcp"):
        logger = logging.getLogger(logger_name)
        logger.addFilter(_FileWatcherFilter())


class _FileWatcherFilter(logging.Filter):
    """Filter out only the misleading 'FileWatcher started' message, keep everything else."""

    def filter(self, record):
        return "FileWatcher started" not in record.getMessage()


# Default tool set — the tools people actually use.
# Set RLM_TOOLS=all to get everything, or comma-separated list to customize.
_DEFAULT_TOOLS = {
    "rlm_start_session",
    "rlm_search_facts",
    "rlm_add_hierarchical_fact",
    "rlm_route_context",
    "rlm_get_hierarchy_stats",
    "rlm_record_causal_decision",
    "rlm_delete_fact",
    "rlm_consolidate_facts",
    "rlm_get_stale_facts",
    "rlm_sync_state",
    "rlm_discover_project",
    "rlm_enterprise_context",
}


def _filter_tools(server):
    """Filter MCP tools to a curated default set.

    Default: 12 essential tools (memory, search, routing, governance).
    RLM_TOOLS=all → keep all tools (no filtering).
    RLM_TOOLS=tool1,tool2 → custom whitelist.
    """
    tools_env = os.environ.get("RLM_TOOLS", "").strip()

    if tools_env.lower() == "all":
        print("  Tools: all (RLM_TOOLS=all)")
        return

    allowed = _DEFAULT_TOOLS
    if tools_env:
        allowed = {t.strip() for t in tools_env.split(",") if t.strip()}

    # MCP SDK v1: _tool_handlers dict; v2+: _tool_manager with remove_tool()
    handlers = getattr(server.mcp, "_tool_handlers", None)
    if handlers is not None:
        before = len(handlers)
        to_remove = [name for name in handlers if name not in allowed]
        for name in to_remove:
            del handlers[name]
        print(f"  Tools: {before} → {len(handlers)} (default set)")
        return

    # v2+: _tool_manager._tools is a dict
    tool_manager = getattr(server.mcp, "_tool_manager", None)
    if tool_manager is not None and hasattr(tool_manager, "_tools"):
        tools_dict = tool_manager._tools
        before = len(tools_dict)
        to_remove = [name for name in tools_dict if name not in allowed]
        for name in to_remove:
            del tools_dict[name]
        print(f"  Tools: {before} → {len(tools_dict)} (default set)")
        return

    print("  WARNING: Cannot filter tools — no known tool storage found")


def _patch_search_facts():
    """Redirect search_facts to use v2 HierarchicalMemoryStore instead of v1 manager.

    Fixes GitHub issue #19: search_facts returns empty because v1 manager.hybrid_search()
    looks at v1 _current_state.facts, but rlm_add_hierarchical_fact stores facts in v2 store.
    This patch makes hybrid_search query the v2 store directly.
    """
    import math
    from datetime import datetime

    from rlm_toolkit.memory_bridge.manager import MemoryBridgeManager

    def _v2_hybrid_search(self, query, top_k=10, semantic_weight=0.5,
                          keyword_weight=0.3, recency_weight=0.2):
        """Hybrid search across v2 hierarchical facts."""
        # Get v2 store from server components (set by _patch_embedding or create_server)
        store = getattr(self, '_v2_store', None)
        if not store:
            # Fallback: try to find v2 store via gc
            import gc
            from rlm_toolkit.memory_bridge.v2.hierarchical import HierarchicalMemoryStore
            for obj in gc.get_objects():
                if isinstance(obj, HierarchicalMemoryStore):
                    store = obj
                    self._v2_store = store
                    break

        if not store:
            print("  [#19] No v2 store found — falling back to v1")
            # Original v1 behavior
            if not self._current_state:
                return []
            query_tokens = set(query.lower().split())
            now = datetime.now()
            scored = []
            for fact in self._current_state.facts:
                if not fact.is_current():
                    continue
                fact_tokens = set(fact.content.lower().split())
                union_size = len(query_tokens | fact_tokens)
                keyword = len(query_tokens & fact_tokens) / max(union_size, 1)
                age_hours = (now - fact.created_at).total_seconds() / 3600
                recency = math.exp(-age_hours / 168)
                score = keyword_weight * keyword + recency_weight * recency
                scored.append((fact, score))
            return sorted(scored, key=lambda x: -x[1])[:top_k]

        # v2 search: get all facts with embeddings, compute hybrid scores
        facts_with_emb = store.get_facts_with_embeddings()
        all_facts = store.get_all_facts() if not facts_with_emb else []

        # Get query embedding
        embedder = getattr(store, '_embedder', None)
        query_embedding = None
        if embedder:
            try:
                import numpy as np
                raw = embedder.encode(query)
                query_embedding = raw.tolist() if hasattr(raw, 'tolist') else list(raw)
            except Exception:
                pass

        query_tokens = set(query.lower().split())
        now = datetime.now()
        scored = []

        def _cosine(a, b):
            import numpy as np
            a, b = np.array(a), np.array(b)
            dot = np.dot(a, b)
            na, nb = np.linalg.norm(a), np.linalg.norm(b)
            return float(dot / (na * nb)) if na > 0 and nb > 0 else 0.0

        # Score facts with embeddings
        for fact, emb in facts_with_emb:
            semantic = _cosine(query_embedding, emb) if query_embedding and emb else 0.0
            fact_tokens = set(fact.content.lower().split())
            union_size = len(query_tokens | fact_tokens)
            keyword = len(query_tokens & fact_tokens) / max(union_size, 1)
            age_hours = (now - fact.created_at).total_seconds() / 3600
            recency = math.exp(-age_hours / 168)
            score = semantic_weight * semantic + keyword_weight * keyword + recency_weight * recency
            scored.append((fact, score))

        # Also score facts without embeddings (keyword + recency only)
        emb_ids = {f.id for f, _ in facts_with_emb}
        for fact in all_facts:
            if fact.id in emb_ids:
                continue
            fact_tokens = set(fact.content.lower().split())
            union_size = len(query_tokens | fact_tokens)
            keyword = len(query_tokens & fact_tokens) / max(union_size, 1)
            age_hours = (now - fact.created_at).total_seconds() / 3600
            recency = math.exp(-age_hours / 168)
            score = keyword_weight * keyword + recency_weight * recency
            scored.append((fact, score))

        # Wrap v2 facts with v1-compatible attributes for serialization
        result = sorted(scored, key=lambda x: -x[1])[:top_k]
        for fact, _ in result:
            if not hasattr(fact, 'entity_type'):
                # Add v1-compatible shim: entity_type with .value
                class _EntityShim:
                    def __init__(self, val):
                        self.value = val
                fact.entity_type = _EntityShim(fact.domain or "fact")
        return result

    MemoryBridgeManager.hybrid_search = _v2_hybrid_search


def _patch_discover_project():
    """Normalize Windows paths for discover_project running inside Linux container.

    Fixes GitHub issue #20: Windows clients send paths like 'd:\\Repos\\BIT'
    which don't exist inside the container. Map to container's /data path.
    """
    try:
        from rlm_toolkit.memory_bridge.v2.coldstart import ColdStartOptimizer
    except ImportError:
        print("  [#20] Cannot patch discover_project — ColdStartOptimizer import failed")
        return

    _original_discover = ColdStartOptimizer.discover_project

    def _patched_discover(self, root=None, task_hint=None, **kwargs):
        import re
        from pathlib import Path

        if root and isinstance(root, (str, Path)):
            path_str = str(root)
            # Detect Windows path (drive letter or backslashes)
            if re.match(r'^[A-Za-z]:[/\\]', path_str) or '\\' in path_str:
                container_root = os.environ.get("RLM_PROJECT_ROOT", "/data")
                print(f"  [#20] Windows path '{path_str}' → container path '{container_root}'")
                root = Path(container_root)

        return _original_discover(self, root=root, task_hint=task_hint, **kwargs)

    ColdStartOptimizer.discover_project = _patched_discover


def _patch_causal_context():
    """Fix causal context retrieval — fallback to recent decisions when LIKE search fails.

    Fixes GitHub issue #22: enterprise_context with include_causal=true returns
    causal_included=false because upstream query_chain uses LIKE substring matching
    which rarely matches natural language queries.

    This patch adds a fallback: if upstream query_chain returns nothing, directly query
    the causal DB for recent decisions and format them as a summary.
    """
    try:
        from rlm_toolkit.memory_bridge.v2.automode import EnterpriseContextBuilder
    except ImportError:
        print("  [#22] Cannot patch causal context — import failed")
        return

    if not hasattr(EnterpriseContextBuilder, '_get_causal_summary'):
        print("  [#22] _get_causal_summary not found — skipping")
        return

    _original_causal = EnterpriseContextBuilder._get_causal_summary

    def _patched_causal(self, query):
        # Try upstream first (LIKE matching)
        try:
            result = _original_causal(self, query)
            if result:
                return result
        except Exception as e:
            logger = logging.getLogger("rlm_workflow")
            logger.warning(f"[#22] Upstream causal query failed: {type(e).__name__}: {e}")

        # Fallback: query recent decisions directly from causal DB
        tracker = getattr(self, 'causal_tracker', None)
        if tracker is None:
            return ""

        try:
            import sqlite3
            db_path = getattr(tracker, 'db_path', None)
            if not db_path or not db_path.exists():
                return ""

            with sqlite3.connect(db_path) as conn:
                conn.row_factory = sqlite3.Row
                # Get recent decisions, keyword-match by splitting query words
                words = [w.lower() for w in query.split() if len(w) > 2]
                if words:
                    conditions = " OR ".join(["LOWER(content) LIKE ?" for _ in words])
                    params = [f"%{w}%" for w in words]
                    sql = f"""
                        SELECT id, content, created_at FROM causal_nodes
                        WHERE node_type = 'decision' AND ({conditions})
                        ORDER BY created_at DESC LIMIT 5
                    """
                    rows = conn.execute(sql, params).fetchall()
                else:
                    rows = conn.execute("""
                        SELECT id, content, created_at FROM causal_nodes
                        WHERE node_type = 'decision'
                        ORDER BY created_at DESC LIMIT 5
                    """).fetchall()

                if not rows:
                    return ""

                parts = ["## Recent Decisions"]
                for row in rows:
                    parts.append(f"- [{row['created_at'][:16]}] {row['content']}")

                    # Get reasons linked to THIS specific decision by ID (not content)
                    # Edge direction: reason --justifies--> decision (from_id=reason, to_id=decision)
                    reason_rows = conn.execute("""
                        SELECT cn.content FROM causal_nodes cn
                        JOIN causal_edges ce ON ce.from_id = cn.id
                        WHERE ce.to_id = ?
                        AND cn.node_type = 'reason'
                        LIMIT 3
                    """, [row['id']]).fetchall()
                    if reason_rows:
                        parts.append(f"  Reasons: {', '.join(r['content'] for r in reason_rows)}")

                return "\n".join(parts)

        except Exception as e:
            logger = logging.getLogger("rlm_workflow")
            logger.warning(f"[#22] Causal fallback query failed: {type(e).__name__}: {e}")
            return ""

    EnterpriseContextBuilder._get_causal_summary = _patched_causal


def _patch_project_overview():
    """Suppress noisy fingerprint in project overview and improve Unknown project display.

    Fixes GitHub issue #23: route_context and enterprise_context show
    'data is a Unknown project' with noisy __FINGERPRINT__ content.
    """
    try:
        from rlm_toolkit.memory_bridge.v2.automode import EnterpriseContextBuilder
    except ImportError:
        print("  [#23] Cannot patch project overview — import failed")
        return

    if not hasattr(EnterpriseContextBuilder, '_get_project_overview'):
        print("  [#23] _get_project_overview not found — skipping")
        return

    _original_overview = EnterpriseContextBuilder._get_project_overview

    def _patched_overview(self):
        result = _original_overview(self)
        if not result:
            return result
        lines = result.split('\n')
        cleaned = [
            line for line in lines
            if '__FINGERPRINT__' not in line
            and 'Unknown project' not in line
        ]
        return '\n'.join(cleaned).strip()

    EnterpriseContextBuilder._get_project_overview = _patched_overview

    # Also patch to_injection_string to filter fingerprint/Unknown from L0 facts
    from rlm_toolkit.memory_bridge.v2.automode import EnterpriseContext

    _original_inject = EnterpriseContext.to_injection_string

    def _patched_inject(self):
        result = _original_inject(self)
        lines = result.split('\n')
        cleaned = [
            line for line in lines
            if '__FINGERPRINT__' not in line
            and 'is a Unknown project' not in line
        ]
        return '\n'.join(cleaned)

    EnterpriseContext.to_injection_string = _patched_inject


def _patch_prevent_default_embedding():
    """Set the correct default model BEFORE upstream loads it.

    Fixes GitHub issue #24: upstream hardcodes DEFAULT_MODEL = "all-MiniLM-L6-v2".
    Instead of intercepting SentenceTransformer constructor, we simply change the
    DEFAULT_MODEL constant so upstream loads the right model on first try.

    For ollama/openai providers: stub SentenceTransformer entirely since embedding
    happens via HTTP API, not local model.
    """
    model_name = os.environ.get("RLM_EMBEDDING_MODEL", "")
    if not model_name:
        return

    provider = os.environ.get("RLM_EMBEDDING_PROVIDER", "").lower()

    # For local SentenceTransformer: change DEFAULT_MODEL so upstream loads the right model.
    # For ollama/openai: DON'T change DEFAULT_MODEL (model name format incompatible with ST).
    if provider not in ("ollama", "openai"):
        patched = []
        for mod_path in [
            "rlm_toolkit.retrieval.embeddings",
            "rlm_toolkit.memory_bridge.v2.embeddings",
        ]:
            try:
                import importlib
                mod = importlib.import_module(mod_path)
                for cls_name in dir(mod):
                    cls = getattr(mod, cls_name)
                    if isinstance(cls, type) and hasattr(cls, 'DEFAULT_MODEL'):
                        old = cls.DEFAULT_MODEL
                        cls.DEFAULT_MODEL = model_name
                        patched.append(f"{cls_name}")
            except ImportError:
                pass
        if patched:
            print(f"  [#24] DEFAULT_MODEL → '{model_name}' in: {', '.join(patched)}")

    # server.py:140 hardcodes SentenceTransformer("all-MiniLM-L6-v2") as a string literal.
    # DEFAULT_MODEL patching doesn't help there. We intercept SentenceTransformer to either:
    # - ollama/openai: stub it (embedding via HTTP API, not local model)
    # - local ST: replace the model name on first call
    import sentence_transformers as st_module
    _original_st_class = st_module.SentenceTransformer

    # Singleton cache — one instance reused for all SentenceTransformer calls (#28)
    _singleton_cache = {}

    if provider in ("ollama", "openai"):
        class _StubEmbedder:
            """Lightweight stub — real embedding happens via HTTP API."""
            def __init__(self, model_name_or_path=None, **kwargs):
                self._dim = 384

            def encode(self, sentences, **kwargs):
                import numpy as np
                if isinstance(sentences, str):
                    return np.zeros(self._dim, dtype=np.float64)
                return np.zeros((len(sentences), self._dim), dtype=np.float64)

            def get_sentence_embedding_dimension(self):
                return self._dim

        class _InterceptedST:
            """Intercept all SentenceTransformer calls — return singleton stub (#28)."""
            def __new__(cls, model_name_or_path=None, **kwargs):
                if model_name_or_path == model_name:
                    # This is our own _patch_embedding call — let it through
                    return _original_st_class(model_name_or_path, **kwargs)
                # Return cached stub — one instance for all callers (#28)
                if "stub" not in _singleton_cache:
                    _singleton_cache["stub"] = _StubEmbedder(model_name_or_path, **kwargs)
                    print(f"  [#24] Stubbing '{model_name_or_path}' (will use {provider}:{model_name})")
                return _singleton_cache["stub"]
    else:
        class _InterceptedST:
            """Redirect any hardcoded model name to the configured one — singleton (#28)."""
            def __new__(cls, model_name_or_path=None, **kwargs):
                actual = model_name if model_name_or_path != model_name else model_name_or_path
                # Return cached model — one load, all callers share (#28)
                if actual in _singleton_cache:
                    return _singleton_cache[actual]
                if actual != model_name_or_path:
                    print(f"  [#24] '{model_name_or_path}' → '{actual}'")
                instance = _original_st_class(actual, **kwargs)
                _singleton_cache[actual] = instance
                return instance

    # Patch everywhere: main module + all modules that did `from ... import SentenceTransformer`
    st_module.SentenceTransformer = _InterceptedST
    for mod_path in [
        "rlm_toolkit.retrieval.embeddings",
        "rlm_toolkit.memory_bridge.v2.embeddings",
        "rlm_toolkit.mcp.server",
    ]:
        try:
            import importlib
            mod = importlib.import_module(mod_path)
            if hasattr(mod, 'SentenceTransformer'):
                mod.SentenceTransformer = _InterceptedST
        except ImportError:
            pass
    print(f"  [#24] SentenceTransformer intercepted — all loads use '{model_name}'")


def main():
    parser = argparse.ArgumentParser(description="RLM-Toolkit MCP server launcher")
    parser.add_argument("--host", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=8200)
    parser.add_argument("--transport", default="sse", choices=["stdio", "sse", "streamable-http"])
    args = parser.parse_args()

    # Ensure RLM_PROJECT_ROOT is set before import (ContextManager reads it at init)
    os.environ.setdefault("RLM_PROJECT_ROOT", "/data")

    # Suppress misleading upstream logs (#17) — before server creation
    _suppress_misleading_logs()

    # Apply class-level patches BEFORE creating server
    _patch_session_restore()          # #8/#7/#18/#21
    _patch_search_facts()             # #19
    _patch_discover_project()         # #20
    _patch_causal_context()           # #22
    _patch_project_overview()         # #23
    _patch_prevent_default_embedding()  # #24 — must be before create_server()

    from rlm_toolkit.mcp.server import create_server

    server = create_server()

    # Apply embedding override AFTER creating server (instance-level patch)
    _patch_embedding(server)

    # Filter tools to allowed set (if RLM_TOOLS env var is set)
    _filter_tools(server)

    server.mcp.settings.host = args.host
    server.mcp.settings.port = args.port

    # Disable host validation — container is network-exposed, not localhost-only
    server.mcp.settings.transport_security = None

    server.mcp.run(transport=args.transport)


if __name__ == "__main__":
    main()
