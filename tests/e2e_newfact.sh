#!/bin/bash
# E2E: new fact -> card embedding + FTS row + RRF retrieval -> delete cleans FTS
set -e
URL=http://localhost:8250/mcp
H_INIT=$(mktemp)
curl -s -D "$H_INIT" -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"e2e","version":"1.0"}}}' -o /dev/null
SID=$(grep -i '^mcp-session-id:' "$H_INIT" | tr -d '\r' | awk '{print $2}')
curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' -o /dev/null
call() { curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" -d "$1"; }

echo "--- add fact ---"
ADD=$(call '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"rlm_add_hierarchical_fact","arguments":{"content":"quixzel projector calibration runbook: drain the flux buffer before recalibrating the quixzel array, otherwise readings drift","level":1,"domain":"testdomain"}}}')
echo "$ADD" | head -c 400; echo
FID=$(echo "$ADD" | python3 -c 'import json,sys,re; t=json.load(sys.stdin)["result"]["content"][0]["text"]; m=re.search(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", t); print(m.group(0))')
echo "fact_id: $FID"

sleep 1
echo "--- DB checks: embedding model + FTS row ---"
sqlite3 /data/.rlm/memory/memory_bridge_v2.db "SELECT model_name, length(embedding) FROM embeddings_index WHERE fact_id='$FID';"
sqlite3 /data/.rlm/memory/memory_bridge_v2.db "SELECT fact_id, substr(content,1,40) FROM facts_fts WHERE fact_id='$FID';"

echo "--- route single-word query 'quixzel' (w_fts=2) ---"
call '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rlm_route_context","arguments":{"query":"quixzel","max_tokens":2000}}}' \
  | python3 -c 'import json,sys; t=json.load(sys.stdin)["result"]["content"][0]["text"]; d=json.loads(t); print("found:", "quixzel projector calibration" in d["context"]); print("explanation:", d["routing_explanation"])'

echo "--- delete fact ---"
call "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_delete_fact\",\"arguments\":{\"fact_id\":\"$FID\"}}}" | head -c 300; echo
sleep 1
echo "--- FTS row gone? ---"
sqlite3 /data/.rlm/memory/memory_bridge_v2.db "SELECT COUNT(*) FROM facts_fts WHERE fact_id='$FID';"
sqlite3 /data/.rlm/memory/memory_bridge_v2.db "SELECT COUNT(*) FROM hierarchical_facts WHERE id='$FID';"
