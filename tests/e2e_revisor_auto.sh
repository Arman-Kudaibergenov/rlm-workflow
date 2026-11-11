#!/bin/bash
# Scoped auto test: override ALL built-in TTLs so only domain 'revtest' is a candidate
set -e
URL=http://localhost:8250/mcp
DB=/data/.rlm/memory/memory_bridge_v2.db
H_INIT=$(mktemp)
curl -s -D "$H_INIT" -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"rev3","version":"1.0"}}}' -o /dev/null
SID=$(grep -i '^mcp-session-id:' "$H_INIT" | tr -d '\r' | awk '{print $2}')
curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' -o /dev/null
call() { curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" -d "$1"; }

cp /data/revisor.yaml /tmp/revisor.yaml.saved
cat > /data/revisor.yaml <<'YAML'
default_ttl_days: 9999
ttl_days:
  work_current: 9999
  tasks: 9999
  project_status: 9999
  infrastructure: 9999
  deploy: 9999
  deployment: 9999
  revtest: 30
never:
  - pitfalls
  - decisions
  - revisor
YAML

ADD=$(call '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rlm_add_hierarchical_fact","arguments":{"content":"revisor scoped auto-test fact in revtest domain","level":1,"domain":"revtest"}}}')
FID=$(echo "$ADD" | python3 -c 'import json,sys,re; t=json.load(sys.stdin)["result"]["content"][0]["text"]; print(re.search(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", t).group(0))')
OLD=$(date -d '40 days ago' +%FT%T.%6N)
sqlite3 $DB "UPDATE hierarchical_facts SET created_at='$OLD', valid_from='$OLD' WHERE id='$FID';"
echo "test fact: $FID aged 40d"

T0=$(date +%s)
call '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"rlm_revisor_run","arguments":{"deep":false,"mode":"auto"}}}' \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); print("mode:", d["mode"], "| candidates:", d["candidates"], "| invalidated:", d["invalidated"]); [print("  ", x["action"], x["id"][:8], x["reason"][:60]) for x in d["decisions"]]'
T1=$(date +%s)
echo "run took $((T1-T0))s"
echo "--- flags on test fact (expect 1|1) ---"
sqlite3 $DB "SELECT is_stale, valid_until IS NOT NULL FROM hierarchical_facts WHERE id='$FID';"

echo "--- restore config + cleanup ---"
cp /tmp/revisor.yaml.saved /data/revisor.yaml
sqlite3 $DB "DELETE FROM hierarchical_facts WHERE id='$FID';"
RID=$(sqlite3 $DB "SELECT id FROM hierarchical_facts WHERE domain='revisor' ORDER BY created_at DESC LIMIT 1;")
sqlite3 $DB "DELETE FROM hierarchical_facts WHERE id='$RID';"
sqlite3 $DB "SELECT COUNT(*) FROM hierarchical_facts WHERE domain IN ('revtest','revisor');"
echo "SCOPED AUTO TEST done"
