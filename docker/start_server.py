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
    """Wraps SentenceTransformer to convert numpy float32 → Python float for JSON safety.

    Fixes GitHub issue #13: rlm-toolkit serializes embeddings to JSON, but numpy.float32
    is not JSON-serializable. This wrapper intercepts encode() output and converts to native
    Python floats via .tolist().
    """

    def __init__(self, model):
        self._model = model

    def encode(self, sentences, **kwargs):
        result = self._model.encode(sentences, **kwargs)
        if hasattr(result, "tolist"):
            return result.tolist()
        # Handle edge case: list of numpy arrays
        if isinstance(result, list):
            import numpy as np

            return [r.tolist() if isinstance(r, np.ndarray) else r for r in result]
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
    """Embedding via OpenAI API. Supports text-embedding-3-small/large, text-embedding-ada-002."""

    # Static dimension lookup — avoids paid API call for dimension probe
    _KNOWN_DIMS = {
        "text-embedding-3-small": 1536,
        "text-embedding-3-large": 3072,
        "text-embedding-ada-002": 1536,
    }

    def __init__(self, model_name: str, api_key: str):
        self._model_name = model_name
        self._api_key = api_key
        self._dim = self._KNOWN_DIMS.get(model_name)

    def encode(self, sentences, **kwargs):
        import json
        import urllib.request

        import numpy as np

        if isinstance(sentences, str):
            sentences = [sentences]

        url = "https://api.openai.com/v1/embeddings"
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

    No Float32SafeEmbedder needed for Ollama/OpenAI — they return numpy arrays
    directly, and upstream calls .tolist() which works on numpy natively.
    """
    provider = os.environ.get("RLM_EMBEDDING_PROVIDER", "").lower()

    if provider == "ollama":
        base_url = os.environ.get("OLLAMA_BASE_URL", "http://localhost:11434")
        embedder = OllamaEmbedder(model_name, base_url)
        dim = embedder.get_sentence_embedding_dimension()
        print(f"  Embedding: {model_name} via Ollama at {base_url} (dim={dim})")
        return embedder, dim

    if provider == "openai":
        api_key = os.environ.get("OPENAI_API_KEY", "")
        if not api_key:
            raise ValueError("RLM_EMBEDDING_PROVIDER=openai but OPENAI_API_KEY not set")
        effective_model = model_name or "text-embedding-3-small"
        embedder = OpenAIEmbedder(effective_model, api_key)
        dim = embedder.get_sentence_embedding_dimension()
        print(f"  Embedding: {effective_model} via OpenAI API (dim={dim})")
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
        print("  Embedding: all-MiniLM-L6-v2 (default)")
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

    except Exception as e:
        import sys

        print(f"  ERROR: Embedding override failed for '{model_name}': {e}")
        print(f"  Falling back to default all-MiniLM-L6-v2")
        print(f"  ERROR: RLM_EMBEDDING_MODEL={model_name} failed", file=sys.stderr)


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

    server.mcp.settings.host = args.host
    server.mcp.settings.port = args.port

    # Disable host validation — container is network-exposed, not localhost-only
    server.mcp.settings.transport_security = None

    server.mcp.run(transport=args.transport)


if __name__ == "__main__":
    main()
