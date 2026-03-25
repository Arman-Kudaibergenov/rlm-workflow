"""Thin wrapper to launch rlm-toolkit MCP server with configurable transport and bind address.

Applies monkey-patches for:
- #13: Float32SafeEmbedder wrapper for numpy→Python float conversion
- #5/#4: Embedding model override from RLM_EMBEDDING_MODEL env var
- #8/#7/#18: Session restore — "default" session only, no fallback to latest
- #16: Single embedder instance reused across store and router
- #17: Suppress misleading FileWatcher log from upstream
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
    """Patch MemoryBridgeManager.start_session to restore "default" session when session_id=None.

    Fixes GitHub issues #8, #7, #18: without this patch, restore=True with session_id=None
    either generates a random UUID (empty session) or falls back to latest session (isolation risk).

    Now: only restores named "default" session. If absent, creates new session via upstream.
    """
    from datetime import datetime

    from rlm_toolkit.memory_bridge.manager import MemoryBridgeManager

    _original = MemoryBridgeManager.start_session

    def _patched(self, session_id=None, restore=True):
        if restore and session_id is None:
            # Only restore named "default" session — no fallback to latest (isolation, #18)
            state = self.storage.load_state("default")
            if state:
                state.version += 1
                state.timestamp = datetime.now()
                self._current_state = state
                print(f"  Session restored: 'default' (v{state.version})")
                return state
            print("  No 'default' session found — creating new session")
        return _original(self, session_id=session_id, restore=restore)

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

    # Apply session restore patch BEFORE creating server (class-level patch)
    _patch_session_restore()

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
