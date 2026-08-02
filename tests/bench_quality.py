#!/usr/bin/env python3
"""Routing quality bench: hit@5 against bench_quality.json.

For each question: call rlm_route_context, strip the always-loaded L0
(PROJECT OVERVIEW) section, take the first 5 fact lines, check that
expected substr appears (case-insensitive). Prints per-query HIT/MISS,
miss details, and final hit@5.

Usage: python3 bench_quality.py [bench_quality.json] [--profile NAME]
"""

import json
import os
import sys
import urllib.request

URL = os.environ.get("RLM_URL", "http://localhost:8250/mcp")


def mcp_session():
    req = urllib.request.Request(
        URL,
        data=json.dumps({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                       "clientInfo": {"name": "benchq", "version": "1.0"}},
        }).encode(),
        headers={"Content-Type": "application/json",
                 "Accept": "application/json, text/event-stream"},
    )
    resp = urllib.request.urlopen(req, timeout=30)
    resp.read()
    sid = resp.headers["mcp-session-id"]
    urllib.request.urlopen(urllib.request.Request(
        URL,
        data=json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}).encode(),
        headers={"Content-Type": "application/json",
                 "Accept": "application/json, text/event-stream",
                 "mcp-session-id": sid},
    ), timeout=30).read()
    return sid


def call_tool(sid, name, args, req_id):
    req = urllib.request.Request(
        URL,
        data=json.dumps({"jsonrpc": "2.0", "id": req_id, "method": "tools/call",
                         "params": {"name": name, "arguments": args}}).encode(),
        headers={"Content-Type": "application/json",
                 "Accept": "application/json, text/event-stream",
                 "mcp-session-id": sid},
    )
    d = json.loads(urllib.request.urlopen(req, timeout=60).read())
    return json.loads(d["result"]["content"][0]["text"])


def routed_fact_lines(context):
    """Fact lines outside the always-loaded PROJECT OVERVIEW (L0) section."""
    lines = []
    section = None
    for line in context.splitlines():
        if line.startswith("## "):
            section = line[3:].strip()
            continue
        if line.startswith("---"):
            break
        if line.startswith("- ") and section != "PROJECT OVERVIEW":
            lines.append(line)
    return lines


def main():
    bench_path = sys.argv[1] if len(sys.argv) > 1 and not sys.argv[1].startswith("--") else "bench_quality.json"
    profile = None
    if "--profile" in sys.argv:
        profile = sys.argv[sys.argv.index("--profile") + 1]

    questions = [q for q in json.load(open(bench_path)) if "q" in q]
    sid = mcp_session()

    hits = 0
    misses = []
    for i, item in enumerate(questions, start=2):
        args = {"query": item["q"], "max_tokens": 2500}
        if profile:
            args["profile"] = profile
        try:
            r = call_tool(sid, "rlm_route_context", args, i)
            top5 = routed_fact_lines(r.get("context", ""))[:5]
            ok = item["substr"].lower() in "\n".join(top5).lower()
        except Exception as e:
            top5, ok = [], False
            print(f"  ERROR {item['topic']}: {e}")
        if ok:
            hits += 1
            print(f"HIT   {item['topic']}: {item['q']}")
        else:
            misses.append((item, top5))
            print(f"MISS  {item['topic']}: {item['q']}  (want: {item['substr'][:60]})")

    total = len(questions)
    print(f"\nhit@5 = {hits}/{total} = {hits/total:.2f}" + (f"  [profile={profile}]" if profile else ""))
    if misses:
        print("\n--- miss details (actual top-5 non-L0 lines) ---")
        for item, top5 in misses:
            print(f"* {item['topic']} ({item['q']})")
            for line in top5:
                print(f"    {line[:150]}")
            if not top5:
                print("    <no non-L0 facts returned>")


if __name__ == "__main__":
    main()
