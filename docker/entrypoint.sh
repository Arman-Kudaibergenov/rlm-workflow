#!/bin/sh
set -e

DATA_DIR="${RLM_DATA_DIR:-/data}"
HOST="${RLM_HOST:-0.0.0.0}"
PORT="${RLM_PORT:-8200}"
TRANSPORT="${RLM_TRANSPORT:-sse}"

echo "Starting RLM-Toolkit MCP server"
echo "  Data dir:  $DATA_DIR"
echo "  Listen:    $HOST:$PORT"
echo "  Transport: $TRANSPORT"

mkdir -p "$DATA_DIR"

# RLM-Toolkit reads RLM_PROJECT_ROOT for storage location
export RLM_PROJECT_ROOT="$DATA_DIR"

exec python /app/start_server.py \
    --host "$HOST" \
    --port "$PORT" \
    --transport "$TRANSPORT"
