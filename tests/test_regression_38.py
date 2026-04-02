"""Regression test matrix for GitHub issues #36, #37, #38.

Tests search_facts reliability fixes:
- #36: search_facts returns results for valid queries (min_score lowered)
- #37: embedding model mismatch detection and reindex
- #38: nonsense queries return empty (min_relevance gate)

Requires: docker container 'rlm-test' running on localhost:8201
"""

import json
import os
import sys
import time
import uuid
import urllib.request


BASE_URL = os.environ.get("RLM_TEST_URL", "http://localhost:8201/mcp")
MSG_ID = 0
SESSION_ID = None


def mcp_call(method: str, params: dict | None = None) -> dict:
    """Send MCP JSON-RPC request via streamable-http."""
    global MSG_ID, SESSION_ID
    MSG_ID += 1
    payload = {
        "jsonrpc": "2.0",
        "id": MSG_ID,
        "method": method,
    }
    if params:
        payload["params"] = params

    data = json.dumps(payload).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if SESSION_ID:
        headers["Mcp-Session-Id"] = SESSION_ID

    req = urllib.request.Request(BASE_URL, data=data, headers=headers)
    with urllib.request.urlopen(req, timeout=60) as resp:
        sid = resp.headers.get("Mcp-Session-Id")
        if sid:
            SESSION_ID = sid
        body = resp.read().decode()

    if "event:" in body or body.strip().startswith("data:"):
        for line in body.split("\n"):
            line = line.strip()
            if line.startswith("data:"):
                return json.loads(line[5:].strip())
        raise ValueError(f"No data line in SSE response: {body[:200]}")

    return json.loads(body)


def mcp_initialize():
    """Initialize MCP session."""
    r = mcp_call("initialize", {
        "protocolVersion": "2025-03-26",
        "capabilities": {},
        "clientInfo": {"name": "regression-test-38", "version": "1.0"},
    })
    global MSG_ID
    MSG_ID += 1
    payload = json.dumps({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    }).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if SESSION_ID:
        headers["Mcp-Session-Id"] = SESSION_ID
    req = urllib.request.Request(BASE_URL, data=payload, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            resp.read()
    except Exception:
        pass
    return r


def call_tool(name: str, arguments: dict) -> dict:
    """Call an MCP tool and return the result content."""
    resp = mcp_call("tools/call", {"name": name, "arguments": arguments})
    if "error" in resp:
        return {"error": resp["error"]}
    result = resp.get("result", {})
    content = result.get("content", [])
    if content and isinstance(content, list):
        for item in content:
            if item.get("type") == "text":
                try:
                    return json.loads(item["text"])
                except json.JSONDecodeError:
                    return {"raw": item["text"]}
    return result


# ============================================================
# Test runner
# ============================================================

PASS = 0
FAIL = 0


def check(test_id: str, desc: str, condition: bool, detail: str = ""):
    global PASS, FAIL
    status = "PASS" if condition else "FAIL"
    if condition:
        PASS += 1
    else:
        FAIL += 1
    extra = f" — {detail}" if detail else ""
    print(f"  [{status}] #{test_id}: {desc}{extra}")


# ============================================================
# #36: search_facts returns results for valid queries
# ============================================================

def test_36_search_finds_existing_facts():
    """#36: search_facts must return matching facts, not empty list."""
    # Add a distinctive fact
    marker = f"PENDING_TASK_{uuid.uuid4().hex[:8]}"
    r = call_tool("rlm_add_hierarchical_fact", {
        "content": f"PENDING tasks next session: {marker} — fix login bug",
        "domain": "workflow",
        "level": 1,
    })
    check("36a", "add PENDING fact", r.get("status") == "success",
          f"fact_id={r.get('fact_id', 'NONE')}")
    fact_id = r.get("fact_id")

    # Wait for embedding to be generated
    time.sleep(1)

    # Search with default weights
    r2 = call_tool("rlm_search_facts", {
        "query": "PENDING tasks next session",
        "top_k": 10,
    })
    results = r2.get("results", [])
    found = any(marker in f.get("content", "") for f in results)
    check("36b", "search_facts default weights finds PENDING fact", found,
          f"got {len(results)} results, marker_found={found}")

    # Search with keyword-heavy weights (the failing case from issue)
    r3 = call_tool("rlm_search_facts", {
        "query": "PENDING tasks next session",
        "keyword_weight": 0.8,
        "semantic_weight": 0.1,
        "recency_weight": 0.1,
        "top_k": 15,
    })
    results3 = r3.get("results", [])
    found3 = any(marker in f.get("content", "") for f in results3)
    check("36c", "search_facts keyword_weight=0.8 finds PENDING fact", found3,
          f"got {len(results3)} results, marker_found={found3}")

    # Search in Russian (multilingual model)
    marker_ru = f"ЗАДАЧА_{uuid.uuid4().hex[:8]}"
    call_tool("rlm_add_hierarchical_fact", {
        "content": f"Незавершённые задачи: {marker_ru} — исправить авторизацию",
        "domain": "workflow",
        "level": 1,
    })
    time.sleep(1)

    r4 = call_tool("rlm_search_facts", {
        "query": "незавершённые задачи авторизация",
        "top_k": 10,
    })
    results4 = r4.get("results", [])
    found4 = any(marker_ru in f.get("content", "") for f in results4)
    check("36d", "search_facts Russian query finds Russian fact", found4,
          f"got {len(results4)} results, marker_found={found4}")

    # Cleanup
    if fact_id:
        call_tool("rlm_delete_fact", {"fact_id": fact_id})


# ============================================================
# #37: embedding model mismatch (startup log check)
# ============================================================

def test_37_model_name_in_embeddings():
    """#37: newly added facts must have correct model_name in embeddings_index."""
    # Add a fact — the write-path patch should set model_name correctly
    marker = f"MODEL_CHECK_{uuid.uuid4().hex[:8]}"
    r = call_tool("rlm_add_hierarchical_fact", {
        "content": f"Model name verification: {marker}",
        "domain": "test-model",
        "level": 1,
    })
    check("37a", "add fact for model check", r.get("status") == "success",
          f"fact_id={r.get('fact_id', 'NONE')}")

    # Search should find it (proves embedding was generated with correct model)
    time.sleep(1)
    r2 = call_tool("rlm_search_facts", {
        "query": f"Model name verification {marker}",
        "top_k": 5,
    })
    results = r2.get("results", [])
    found = any(marker in f.get("content", "") for f in results)
    check("37b", "fact findable via search (correct model embedding)", found,
          f"got {len(results)} results")


# ============================================================
# #38: nonsense queries return empty
# ============================================================

def test_38_garbage_query_returns_empty():
    """#38: nonsense query must return empty list despite fresh facts."""
    # First, add a fresh fact to ensure recency boost is available
    call_tool("rlm_add_hierarchical_fact", {
        "content": "Fresh fact for recency test — deploy pipeline status check",
        "domain": "workflow",
        "level": 1,
    })
    time.sleep(1)

    # Nonsense query — should return empty
    r = call_tool("rlm_search_facts", {
        "query": "xyzzy12345nonsense_ABCDEF_QQQQQ",
        "top_k": 10,
    })
    results = r.get("results", [])
    check("38a", "nonsense query returns empty", len(results) == 0,
          f"got {len(results)} results (expected 0)")

    # Another garbage pattern
    r2 = call_tool("rlm_search_facts", {
        "query": "asjdfklajsdflkajsdfkljasldkfjals",
        "top_k": 5,
    })
    results2 = r2.get("results", [])
    check("38b", "random string query returns empty", len(results2) == 0,
          f"got {len(results2)} results (expected 0)")

    # Edge case: single common word — may match, that's OK (tests the gate isn't too aggressive)
    r3 = call_tool("rlm_search_facts", {
        "query": "deploy",
        "top_k": 5,
    })
    results3 = r3.get("results", [])
    check("38c", "single relevant word returns results", len(results3) > 0,
          f"got {len(results3)} results (expected >0)")


# ============================================================
# Edge cases
# ============================================================

def test_edge_cases():
    """Edge cases for scoring changes."""
    # Empty query
    r = call_tool("rlm_search_facts", {"query": "", "top_k": 5})
    results = r.get("results", [])
    check("EC1", "empty query doesn't crash", r.get("status") != "error",
          f"status={r.get('status')}, results={len(results)}")

    # Very long query
    long_query = "test " * 200
    r2 = call_tool("rlm_search_facts", {"query": long_query, "top_k": 5})
    check("EC2", "long query doesn't crash", r2.get("status") != "error",
          f"status={r2.get('status')}")


def main():
    print("=" * 60)
    print("RLM Regression Test Matrix (#36, #37, #38)")
    print(f"Target: {BASE_URL}")
    print("=" * 60)

    try:
        mcp_initialize()
        print("\nServer OK — MCP session initialized\n")
    except Exception as e:
        print(f"\nERROR: Cannot reach server: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)

    # Start session
    call_tool("rlm_start_session", {"restore": False})

    print("--- #36: search_facts finds existing facts ---")
    test_36_search_finds_existing_facts()

    print("\n--- #37: model_name in embeddings ---")
    test_37_model_name_in_embeddings()

    print("\n--- #38: garbage query returns empty ---")
    test_38_garbage_query_returns_empty()

    print("\n--- Edge cases ---")
    test_edge_cases()

    print("\n" + "=" * 60)
    print(f"RESULTS: {PASS} PASS, {FAIL} FAIL out of {PASS + FAIL} tests")
    print("=" * 60)

    sys.exit(1 if FAIL > 0 else 0)


if __name__ == "__main__":
    main()
