#!/usr/bin/env python3
"""Done-when #1: compare tools/list between the Python prod server (:8250)
and the Rust test server (:8251). Names, descriptions, schemas."""
import json
import sys
import urllib.request


def tools_list(url):
    def post(payload, sid=""):
        headers = {"Content-Type": "application/json",
                   "Accept": "application/json, text/event-stream"}
        if sid:
            headers["Mcp-Session-Id"] = sid
        req = urllib.request.Request(url, data=json.dumps(payload).encode(), headers=headers)
        with urllib.request.urlopen(req, timeout=30) as resp:
            new_sid = resp.headers.get("Mcp-Session-Id", sid)
            body = resp.read().decode()
        for line in body.split("\n"):
            if line.strip().startswith("data:"):
                return json.loads(line.strip()[5:].strip()), new_sid
        if not body.strip():
            return {}, new_sid
        return json.loads(body), new_sid

    _, sid = post({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                              "clientInfo": {"name": "cmp", "version": "1.0"}}})
    post({"jsonrpc": "2.0", "method": "notifications/initialized"}, sid)
    r, _ = post({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}, sid)
    return {t["name"]: t for t in r["result"]["tools"]}


py = tools_list("http://127.0.0.1:8250/mcp")
rs = tools_list("http://127.0.0.1:8253/mcp")

print(f"Python tools: {len(py)}, Rust tools: {len(rs)}")
only_py = sorted(set(py) - set(rs))
only_rs = sorted(set(rs) - set(py))
if only_py:
    print("ONLY IN PYTHON:", only_py)
if only_rs:
    print("ONLY IN RUST:", only_rs)

diffs = 0
for name in sorted(set(py) & set(rs)):
    a, b = py[name], rs[name]
    if a.get("description") != b.get("description"):
        diffs += 1
        print(f"DESC DIFF {name}:\n  py: {a.get('description')}\n  rs: {b.get('description')}")
    if a.get("inputSchema") != b.get("inputSchema"):
        diffs += 1
        print(f"SCHEMA DIFF {name}:")
        print("  py:", json.dumps(a.get("inputSchema"), ensure_ascii=False, sort_keys=True))
        print("  rs:", json.dumps(b.get("inputSchema"), ensure_ascii=False, sort_keys=True))
    if a.get("outputSchema") != b.get("outputSchema"):
        diffs += 1
        print(f"OUTPUT SCHEMA DIFF {name}")

same_names = set(py) == set(rs)
print(f"\nNames identical: {same_names}; field diffs: {diffs}")
print("RESULT:", "PASS" if same_names and diffs == 0 and len(rs) == 18 else "FAIL")
sys.exit(0 if same_names and diffs == 0 and len(rs) == 18 else 1)