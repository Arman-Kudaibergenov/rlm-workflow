#!/bin/bash
# E2E #45: bi-temporal as_of + invalidate + soft delete
set -e
URL=http://localhost:8250/mcp
DB=/data/.rlm/memory/memory_bridge_v2.db
H_INIT=$(mktemp)
curl -s -D "$H_INIT" -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"bt","version":"1.0"}}}' -o /dev/null
SID=$(grep -i '^mcp-session-id:' "$H_INIT" | tr -d '\r' | awk '{print $2}')
curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' -o /dev/null
call() { curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" -d "$1"; }
found() { python3 -c "import json,sys; d=json.loads(json.load(sys.stdin)['result']['content'][0]['text']); print('tempzel' in d.get('context',''), '| as_of:', d.get('as_of'), '| facts:', d.get('facts_count'))"; }

echo "--- tools count ---"
call '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | python3 -c 'import json,sys; ts=json.load(sys.stdin)["result"]["tools"]; names=[t["name"] for t in ts]; print(len(ts)); print("invalidate" in str(names), "inactive" in str(names))'

echo "--- add tempzel fact ---"
ADD=$(call '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rlm_add_hierarchical_fact","arguments":{"content":"tempzel valve protocol: purge the tempzel line before winter to avoid resonance lock","level":1,"domain":"testdomain"}}}')
echo "$ADD" | head -c 300; echo
FID=$(echo "$ADD" | python3 -c 'import json,sys,re; t=json.load(sys.stdin)["result"]["content"][0]["text"]; print(re.search(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", t).group(0))')
echo "fact: $FID"
sleep 1

echo "--- route default BEFORE invalidate (expect True) ---"
call '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"rlm_route_context","arguments":{"query":"tempzel","max_tokens":1500}}}' | found

echo "--- invalidate ---"
call "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_invalidate_fact\",\"arguments\":{\"fact_id\":\"$FID\",\"reason\":\"e2e test\"}}}" | head -c 250; echo

echo "--- route default AFTER invalidate (expect False) ---"
call '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"rlm_route_context","arguments":{"query":"tempzel","max_tokens":1500}}}' | found

Y=$(date -d 'yesterday' +%F)
T=$(date -d 'tomorrow' +%F)
echo "--- route as_of=yesterday ($Y) (expect True) ---"
call "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_route_context\",\"arguments\":{\"query\":\"tempzel\",\"max_tokens\":1500,\"as_of\":\"$Y\"}}}" | found
echo "--- route as_of=tomorrow ($T) (expect False) ---"
call "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_route_context\",\"arguments\":{\"query\":\"tempzel\",\"max_tokens\":1500,\"as_of\":\"$T\"}}}" | found

echo "--- rlm_diff since=1h (expect tempzel in staled) ---"
call '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"rlm_diff","arguments":{"since":"1h","limit":10}}}' \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); st=d["groups"]["staled"]; print("staled count:", d["counts"]["staled"], "| tempzel in staled:", any("tempzel" in e["content"] for e in st))'

echo "--- rlm_diff since=7d as_of=yesterday (runs, temporal filter) ---"
call "{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_diff\",\"arguments\":{\"since\":\"7d\",\"limit\":3,\"as_of\":\"$Y\"}}}" \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); print("status:", d["status"], "total:", d["total_changes"], "as_of:", d["as_of"])'

echo "--- rlm_list_inactive (expect tempzel on top) ---"
call '{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"rlm_list_inactive","arguments":{"limit":3}}}' \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); [print(" ", r["valid_until"], r["domain"], r["content"][:60]) for r in d["inactive"]]'

echo "--- soft delete check: add + rlm_delete_fact ---"
ADD2=$(call '{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"rlm_add_hierarchical_fact","arguments":{"content":"delzel soft-delete probe fact","level":1,"domain":"testdomain"}}}')
FID2=$(echo "$ADD2" | python3 -c 'import json,sys,re; t=json.load(sys.stdin)["result"]["content"][0]["text"]; print(re.search(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", t).group(0))')
sleep 1
call "{\"jsonrpc\":\"2.0\",\"id\":13,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_delete_fact\",\"arguments\":{\"fact_id\":\"$FID2\"}}}" | head -c 150; echo
sqlite3 $DB "SELECT is_archived, is_stale, valid_until IS NOT NULL FROM hierarchical_facts WHERE id='$FID2';"
sqlite3 $DB "SELECT COUNT(*) FROM facts_fts WHERE fact_id='$FID2';"

echo "--- cleanup (hard delete test rows) ---"
sqlite3 $DB "DELETE FROM hierarchical_facts WHERE id IN ('$FID','$FID2');"
sqlite3 $DB "SELECT COUNT(*) FROM hierarchical_facts WHERE content LIKE '%tempzel%' OR content LIKE '%delzel%';"
sqlite3 $DB "SELECT COUNT(*) FROM facts_fts WHERE content LIKE '%tempzel%' OR content LIKE '%delzel%';"
echo "E2E done"
