#!/bin/bash
# Corrected E2E #45: as_of must be BETWEEN created_at and valid_until
set -e
URL=http://localhost:8250/mcp
DB=/data/.rlm/memory/memory_bridge_v2.db
H_INIT=$(mktemp)
curl -s -D "$H_INIT" -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"bt2","version":"1.0"}}}' -o /dev/null
SID=$(grep -i '^mcp-session-id:' "$H_INIT" | tr -d '\r' | awk '{print $2}')
curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' -o /dev/null
call() { curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" -d "$1"; }
found() { python3 -c "import json,sys; d=json.loads(json.load(sys.stdin)['result']['content'][0]['text']); print('zorbak' in d.get('context',''))"; }

echo "--- cleanup leftover tempzel from failed run ---"
sqlite3 $DB "DELETE FROM hierarchical_facts WHERE content LIKE '%tempzel%';"
sqlite3 $DB "SELECT COUNT(*) FROM hierarchical_facts WHERE content LIKE '%tempzel%'; SELECT COUNT(*) FROM facts_fts WHERE content LIKE '%tempzel%';"

echo "--- add zorbak fact ---"
ADD=$(call '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rlm_add_hierarchical_fact","arguments":{"content":"zorbak gasket mnemonic: tighten the zorbak bolts crosswise or the seal sings","level":1,"domain":"testdomain"}}}')
FID=$(echo "$ADD" | python3 -c 'import json,sys,re; t=json.load(sys.stdin)["result"]["content"][0]["text"]; print(re.search(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", t).group(0))')
echo "fact: $FID"
sleep 1
MID=$(date -u +%FT%T)   # moment after creation, before invalidation
sleep 2

echo "--- invalidate ---"
call "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_invalidate_fact\",\"arguments\":{\"fact_id\":\"$FID\",\"reason\":\"e2e\"}}}" > /dev/null

echo "--- default (expect False) ---"
call '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"rlm_route_context","arguments":{"query":"zorbak","max_tokens":1500}}}' | found
echo "--- as_of=$MID between create and invalidate (expect True) ---"
call "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_route_context\",\"arguments\":{\"query\":\"zorbak\",\"max_tokens\":1500,\"as_of\":\"$MID\"}}}" | found
echo "--- as_of=tomorrow (expect False) ---"
call "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_route_context\",\"arguments\":{\"query\":\"zorbak\",\"max_tokens\":1500,\"as_of\":\"$(date -d tomorrow +%F)\"}}}" | found

echo "--- cleanup ---"
sqlite3 $DB "DELETE FROM hierarchical_facts WHERE id='$FID';"
sqlite3 $DB "SELECT COUNT(*) FROM hierarchical_facts WHERE content LIKE '%zorbak%'; SELECT COUNT(*) FROM facts_fts WHERE content LIKE '%zorbak%';"
echo "E2E2 done"
