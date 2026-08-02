#!/bin/bash
# Benchmark route_context: top facts + latency per query. Usage: bench_route.sh <outdir> <label>
set -e
URL=${RLM_URL:-http://localhost:8250/mcp}
OUT=$1
LABEL=$2
mkdir -p "$OUT"
H_INIT=$(mktemp)

curl -s -D "$H_INIT" -X POST "$URL" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"bench","version":"1.0"}}}' -o /dev/null
SID=$(grep -i '^mcp-session-id:' "$H_INIT" | tr -d '\r' | awk '{print $2}')
curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' -o /dev/null

QUERIES=(
  "project architecture"
  "deployment process"
  "vpn setup"
  "database backup"
  "monitoring alerts"
  "api integration"
  "security policy"
  "release checklist"
  "incident postmortem"
  "service discovery"
)

echo "query,time_s" > "$OUT/$LABEL.latency.csv"
i=0
for q in "${QUERIES[@]}"; do
  i=$((i+1))
  REQ=$(python3 -c "import json,sys; print(json.dumps({'jsonrpc':'2.0','id':$i,'method':'tools/call','params':{'name':'rlm_route_context','arguments':{'query':sys.argv[1],'max_tokens':2000}}}))" "$q")
  TIME=$(curl -s -w '%{time_total}' -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" -d "$REQ" -o "$OUT/$LABEL.$i.json")
  echo "$q,$TIME" >> "$OUT/$LABEL.latency.csv"
  # compact summary: first 5 fact lines
  python3 -c "
import json
d = json.load(open('$OUT/$LABEL.$i.json'))
r = d['result']['content'][0]['text']
try:
    inner = json.loads(r)
    ctx = inner.get('context','')
    lines = [l for l in ctx.splitlines() if l.startswith('- ')][:5]
    print('=== $q ===')
    print('facts_count:', inner.get('facts_count'), 'confidence:', inner.get('routing_confidence'), 'reranked:', inner.get('reranked','n/a'))
    for l in lines: print(l[:180])
except Exception as e:
    print('PARSE-ERR', e, r[:200])
" > "$OUT/$LABEL.$i.txt"
done
echo "done: $OUT/$LABEL"
