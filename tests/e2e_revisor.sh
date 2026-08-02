#!/bin/bash
# Revisor E2E: report run, then auto run on a synthetic 40-day-old fact, cleanup
set -e
URL=http://localhost:8250/mcp
DB=/data/.rlm/memory/memory_bridge_v2.db
H_INIT=$(mktemp)
curl -s -D "$H_INIT" -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"rev","version":"1.0"}}}' -o /dev/null
SID=$(grep -i '^mcp-session-id:' "$H_INIT" | tr -d '\r' | awk '{print $2}')
curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' -o /dev/null
call() { curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" -d "$1"; }

echo "--- tools count (expect 18) ---"
call '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | python3 -c 'import json,sys; ts=json.load(sys.stdin)["result"]["tools"]; print(len(ts), "revisor" in str([t["name"] for t in ts]))'

echo "--- revisor_run report ---"
call '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rlm_revisor_run","arguments":{"deep":false}}}' \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); print("mode:", d["mode"], "| live:", d["live_facts"], "| candidates:", d["candidates"], "| invalidated:", d["invalidated"], "| ping-alive:", d["ping_alive_skipped"]); [print("  ", x["action"], x["id"][:8], x["reason"][:70]) for x in d["decisions"][:6]]'

echo "--- report fact in domain revisor? ---"
sleep 1
sqlite3 $DB "SELECT substr(content,1,120) FROM hierarchical_facts WHERE domain='revisor' ORDER BY created_at DESC LIMIT 1;"

echo "--- auto test: synthetic 40-day-old tasks fact ---"
ADD=$(call '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"rlm_add_hierarchical_fact","arguments":{"content":"revisor-test: old tasks fact that should be auto-invalidated by the revisor","level":1,"domain":"tasks"}}}')
FID=$(echo "$ADD" | python3 -c 'import json,sys,re; t=json.load(sys.stdin)["result"]["content"][0]["text"]; print(re.search(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", t).group(0))')
OLD=$(date -d '40 days ago' +%FT%T.%6N)
sqlite3 $DB "UPDATE hierarchical_facts SET created_at='$OLD', valid_from='$OLD' WHERE id='$FID';"
echo "test fact $FID aged to $OLD"

echo "--- revisor_run auto ---"
call '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"rlm_revisor_run","arguments":{"deep":false,"mode":"auto"}}}' \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); print("mode:", d["mode"], "| candidates:", d["candidates"], "| invalidated:", d["invalidated"]); mine=[x for x in d["decisions"] if "ttl 30d" in x["reason"] or True]; [print("  ", x["action"], x["id"][:8], x["reason"][:60]) for x in d["decisions"][:8]]'
sqlite3 $DB "SELECT is_stale, valid_until IS NOT NULL FROM hierarchical_facts WHERE id='$FID';"

echo "--- cleanup: hard delete test fact ---"
sqlite3 $DB "DELETE FROM hierarchical_facts WHERE id='$FID';"
sqlite3 $DB "SELECT COUNT(*) FROM hierarchical_facts WHERE content LIKE 'revisor-test:%';"
echo "REVISOR E2E done"
