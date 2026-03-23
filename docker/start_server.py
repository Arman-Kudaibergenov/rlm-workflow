"""Thin wrapper to launch rlm-toolkit MCP server with configurable transport and bind address.

Applies monkey-patches for:
- #5/#4: Embedding model override from RLM_EMBEDDING_MODEL env var
- #8/#7: Session restore fallback to "default" or most recent session
"""

import argparse
import os


def _patch_session_restore():
    """Patch MemoryBridgeManager.start_session to restore last session when session_id=None.

    Fixes GitHub issues #8 and #7: without this patch, restore=True with session_id=None
    generates a random UUID, tries to load it (fails), and creates an empty session.
    """
    from rlm_toolkit.memory_bridge.manager import MemoryBridgeManager
    from datetime import datetime

    _original = MemoryBridgeManager.start_session

    def _patched(self, session_id=None, restore=True):
        if restore and session_id is None:
            # Try "default" session first
            state = self.storage.load_state("default")
            if state:
                state.version += 1
                state.timestamp = datetime.now()
                self._current_state = state
                print(f"  Session restored: 'default' (v{state.version})")
                return state
            # Else try most recent session from storage (already sorted DESC by last_updated)
            if hasattr(self.storage, "list_sessions"):
                sessions = self.storage.list_sessions()
                if sessions:
                    latest_id = sessions[0]["session_id"]
                    state = self.storage.load_state(latest_id)
                    if state:
                        state.version += 1
                        state.timestamp = datetime.now()
                        self._current_state = state
                        print(f"  Session restored: '{latest_id}' (v{state.version})")
                        return state
        return _original(self, session_id=session_id, restore=restore)

    MemoryBridgeManager.start_session = _patched


def _patch_embedding(server):
    """Override embedding model from RLM_EMBEDDING_MODEL env var.

    Fixes GitHub issues #5 and #4: rlm-toolkit hardcodes all-MiniLM-L6-v2,
    ignoring RLM_EMBEDDING_MODEL env var.

    Patches both HierarchicalMemoryStore embedder AND SemanticRouter's EmbeddingService
    to avoid dimension mismatch between stored embeddings and search queries.
    """
    model_name = os.environ.get("RLM_EMBEDDING_MODEL", "")
    if not model_name:
        print("  Embedding: all-MiniLM-L6-v2 (default)")
        return

    try:
        from sentence_transformers import SentenceTransformer
        from rlm_toolkit.memory_bridge.v2.embeddings import EmbeddingService

        embedder = SentenceTransformer(model_name)
        dim = embedder.get_sentence_embedding_dimension()

        # Patch store embedder
        server.memory_bridge_v2_store.set_embedder(embedder)

        # Patch router's EmbeddingService to use same model
        components = getattr(server, "memory_bridge_v2_components", {})
        router = components.get("router")
        if router:
            router.embedding_service = EmbeddingService(model_name=model_name)
            # Force model load so it's ready
            _ = router.embedding_service.model

        print(f"  Embedding: {model_name} (dim={dim}, from RLM_EMBEDDING_MODEL)")
    except Exception as e:
        print(f"  ERROR: Embedding override failed for '{model_name}': {e}")
        print(f"  Falling back to default all-MiniLM-L6-v2")
        import sys
        print(f"  ERROR: RLM_EMBEDDING_MODEL={model_name} failed", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description="RLM-Toolkit MCP server launcher")
    parser.add_argument("--host", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=8200)
    parser.add_argument("--transport", default="sse", choices=["stdio", "sse", "streamable-http"])
    args = parser.parse_args()

    # Ensure RLM_PROJECT_ROOT is set before import (ContextManager reads it at init)
    os.environ.setdefault("RLM_PROJECT_ROOT", "/data")

    # Apply session restore patch BEFORE creating server (class-level patch)
    _patch_session_restore()

    from rlm_toolkit.mcp.server import create_server

    server = create_server()

    # Apply embedding override AFTER creating server (instance-level patch)
    _patch_embedding(server)

    server.mcp.settings.host = args.host
    server.mcp.settings.port = args.port
    server.mcp.run(transport=args.transport)


if __name__ == "__main__":
    main()
