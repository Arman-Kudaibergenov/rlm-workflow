#!/bin/sh
set -e

DATA_DIR="${RLM_DATA_DIR:-/data}"
HOST="${RLM_HOST:-0.0.0.0}"
PORT="${RLM_PORT:-8200}"
EMBEDDING_PROVIDER="${RLM_EMBEDDING_PROVIDER:-fastembed}"
EMBEDDING_MODEL="${RLM_EMBEDDING_MODEL:-}"

echo "Starting RLM-Toolkit MCP server"
echo "  Data dir:  $DATA_DIR"
echo "  Listen:    $HOST:$PORT"
echo "  Embeddings: $EMBEDDING_PROVIDER"

mkdir -p "$DATA_DIR"

# Build args
ARGS="--host $HOST --port $PORT --data-dir $DATA_DIR"

if [ -n "$EMBEDDING_PROVIDER" ]; then
    ARGS="$ARGS --embedding-provider $EMBEDDING_PROVIDER"
fi

if [ -n "$EMBEDDING_MODEL" ]; then
    ARGS="$ARGS --embedding-model $EMBEDDING_MODEL"
fi

exec rlm-server $ARGS
