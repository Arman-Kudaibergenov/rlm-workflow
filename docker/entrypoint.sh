#!/bin/sh
set -e

DATA_DIR="${RLM_DATA_DIR:-/data}"
HOST="${RLM_HOST:-0.0.0.0}"
PORT="${RLM_PORT:-8200}"
TRANSPORT="${RLM_TRANSPORT:-streamable-http}"

# Default to multilingual model (supports Russian, English, etc.)
export RLM_EMBEDDING_MODEL="${RLM_EMBEDDING_MODEL:-paraphrase-multilingual-MiniLM-L12-v2}"
EMBEDDING="$RLM_EMBEDDING_MODEL"

echo "Starting RLM-Toolkit MCP server"
echo "  Data dir:  $DATA_DIR"
echo "  Listen:    $HOST:$PORT"
echo "  Transport: $TRANSPORT"
echo "  Embedding: $EMBEDDING"

mkdir -p "$DATA_DIR"

# RLM-Toolkit reads RLM_PROJECT_ROOT for storage location
# Respect user-provided value (e.g., /workspace bind mount); fallback to DATA_DIR (#14)
export RLM_PROJECT_ROOT="${RLM_PROJECT_ROOT:-$DATA_DIR}"

exec python /app/start_server.py \
    --host "$HOST" \
    --port "$PORT" \
    --transport "$TRANSPORT"
