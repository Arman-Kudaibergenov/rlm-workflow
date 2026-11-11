#!/usr/bin/env python3
"""Smoke-test the tools not covered by the regression suites, against both servers."""
import json
import sys
import urllib.request


class S:
    def __init__(self, url):
        self.url = url
        self.sid = ""
        self.n = 0
        r = self.post({"jsonrpc": "2.0", "id": self.nid(), "method": "initialize",
                       "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                                  "clientInfo": {"name": "smoke", "version": "1"}}})
        self.sid = r[1]
        self.post({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def nid(self):
        self.n += 1
        return self.n

    def post(self, payload):
        h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        if self.sid:
            h["Mcp-Session-Id"] = self.sid
        req = urllib.request.Request(self.url, data=json.dumps(payload).encode(), headers=h)
        with urllib.request.urlopen(req, timeout=120) as resp:
            sid = resp.headers.get("Mcp-Session-Id", self.sid)
            body = resp.read().decode()
        for line in body.split("\n"):
            if line.strip().startswith("data:"):
                return json.loads(line.strip()[5:].strip()), sid
        if not body.strip():
            return {}, sid
        return json.loads(body), sid

    def call(self, name, args):
        r, _ = self.post({"jsonrpc": "2.0", "id": self.nid(), "method": "tools/call",
                          "params": {"name": name, "arguments": args}})
        res = r.get("result", {})
        txt = res.get("content", [{}])[0].get("text", "{}")
        try:
            return json.loads(txt)
        except Exception:
            return {"raw": txt}


def smoke(url, label):
    s = S(url)
    out = {}
    out["diff"] = s.call("rlm_diff", {"since": "7d", "limit": 5})
    out["list_inactive"] = s.call("rlm_list_inactive", {"limit": 3})
    out["stats"] = s.call("rlm_get_hierarchy_stats", {})
    # add a throwaway fact, invalidate it, check it appears in list_inactive
    add = s.call("rlm_add_hierarchical_fact",
                 {"content": f"SMOKE_INVALIDATE_{label}: temp fact for window close", "domain": "smoke_test", "level": 2})
    fid = add.get("fact_id")
    out["add_status"] = add.get("status")
    if fid:
        out["invalidate"] = s.call("rlm_invalidate_fact", {"fact_id": fid, "reason": "smoke test"})
        out["delete"] = s.call("rlm_delete_fact", {"fact_id": fid})
    out["revisor"] = s.call("rlm_revisor_run", {"deep": False, "mode": "report"})
    out["consolidate"] = s.call("rlm_consolidate_facts", {"min_facts": 5})
    out["profiles"] = s.call("rlm_list_profiles", {})
    out["session"] = s.call("rlm_start_session", {"restore": True})
    out["sync"] = s.call("rlm_sync_state", {})
    return out


py = smoke("http://127.0.0.1:8252/mcp", "py")
rs = smoke("http://127.0.0.1:8253/mcp", "rs")

for key in py:
    p, r = py[key], rs[key]
    if isinstance(p, dict) and isinstance(r, dict):
        pk, rk = set(p.keys()), set(r.keys())
        status_ok = p.get("status") == r.get("status")
        keys_ok = pk == rk
        print(f"{key}: status py={p.get('status')} rs={r.get('status')} | keys_match={keys_ok}")
        if not keys_ok:
            print(f"   only_py={sorted(pk - rk)} only_rs={sorted(rk - pk)}")
    else:
        print(f"{key}: py={p} rs={r}")

# detailed counts
print("\n--- counts ---")
for label, o in [("py", py), ("rs", rs)]:
    d = o["diff"]
    print(f"{label} diff: total={d.get('total_changes')} counts={d.get('counts')} returned={d.get('returned')}")
    print(f"{label} inactive: count={o['list_inactive'].get('count')}")
    print(f"{label} revisor: live={o['revisor'].get('live_facts')} candidates={o['revisor'].get('candidates')} invalidated={o['revisor'].get('invalidated')}")
    c = o["consolidate"]
    print(f"{label} consolidate: merged={c.get('merged_count')} promoted={c.get('promoted_count')} archived={c.get('archived_count')}")
    print(f"{label} invalidate: {o.get('invalidate', {}).get('status')} delete: {o.get('delete', {}).get('status')}")
print("SMOKE_DONE")