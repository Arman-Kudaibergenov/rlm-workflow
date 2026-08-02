#!/bin/bash
# Verify #44: profiles surface + diff between profiles
set -e
URL=${RLM_URL:-http://localhost:8250/mcp}
H_INIT=$(mktemp)
curl -s -D "$H_INIT" -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"p","version":"1.0"}}}' -o /dev/null
SID=$(grep -i '^mcp-session-id:' "$H_INIT" | tr -d '\r' | awk '{print $2}')
curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' -o /dev/null
call() { curl -s -X POST "$URL" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -H "mcp-session-id: $SID" -d "$1"; }

echo "--- tool count ---"
call '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | python3 -c 'import json,sys; ts=json.load(sys.stdin)["result"]["tools"]; print(len(ts), sorted(t["name"] for t in ts))'

echo "--- rlm_list_profiles ---"
call '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rlm_list_profiles","arguments":{}}}' \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); [print(k, "->", v["domains"]) for k,v in d["profiles"].items()]'

for P in dev-1c em infra bogus; do
  echo "--- route example profile=$P ---"
  call "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"rlm_route_context\",\"arguments\":{\"query\":\"example project\",\"max_tokens\":1500,\"profile\":\"$P\"}}}" \
    > /tmp/prof_$P.json
  python3 -c "
import json
d = json.loads(json.load(open('/tmp/prof_$P.json'))['result']['content'][0]['text'])
print('profile:', d.get('profile'), '| warning:', d.get('warning', '-'))
lines = []
section = None
for line in d.get('context','').splitlines():
    if line.startswith('## '): section = line[3:].strip(); continue
    if line.startswith('---'): break
    if line.startswith('- ') and section != 'PROJECT OVERVIEW': lines.append(line)
for l in lines[:5]: print('  ', l[:130])
"
done

echo "--- enterprise_context profile=em ---"
call '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"rlm_enterprise_context","arguments":{"query":"example project status","max_tokens":1500,"include_causal":false,"profile":"em"}}}' \
  | python3 -c 'import json,sys; d=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"]); print("profile:", d.get("profile")); print(d.get("context","")[:600])'
