"""Regression test matrix for GitHub issue #35.

Tests all 6 fixes (#29-#34) via real MCP calls to running container.
Requires: docker container 'rlm-test' running on localhost:8201
"""

import json
import os
import urllib.request
import sys
import time
import uuid


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
        # Capture session ID from response headers
        sid = resp.headers.get("Mcp-Session-Id")
        if sid:
            SESSION_ID = sid
        body = resp.read().decode()

    # Handle SSE format
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
        "clientInfo": {"name": "regression-test", "version": "1.0"},
    })
    # Send initialized notification
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
        pass  # Notification may not return response
    return r


def call_tool(name: str, arguments: dict) -> dict:
    """Call an MCP tool and return the result content."""
    resp = mcp_call("tools/call", {"name": name, "arguments": arguments})
    if "error" in resp:
        return {"error": resp["error"]}
    result = resp.get("result", {})
    # Extract text content from MCP response
    content = result.get("content", [])
    if content and isinstance(content, list):
        for item in content:
            if item.get("type") == "text":
                try:
                    return json.loads(item["text"])
                except json.JSONDecodeError:
                    return {"raw": item["text"]}
    return result


def list_tools() -> list[str]:
    """Get list of available tool names."""
    resp = mcp_call("tools/list")
    result = resp.get("result", {})
    tools = result.get("tools", [])
    return [t["name"] for t in tools]


# ============================================================
# Test functions
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


def test_01_list_tools():
    """#29: rlm_get_facts_by_domain in tool list."""
    tools = list_tools()
    check("29", "rlm_get_facts_by_domain in ListTools", "rlm_get_facts_by_domain" in tools,
          f"found {len(tools)} tools")


def test_02_start_session():
    """Test 1-2: session start/restore."""
    # Start fresh
    r = call_tool("rlm_start_session", {"restore": False})
    check("T1", "start_session restore=false", r.get("status") != "error",
          f"status={r.get('status')}")

    # Restore
    r2 = call_tool("rlm_start_session", {"restore": True})
    check("T2", "start_session restore=true", r2.get("status") != "error",
          f"restored={r2.get('restored')}")


def test_03_add_fact():
    """Test 3-4: add facts."""
    r = call_tool("rlm_add_hierarchical_fact", {
        "content": f"TEST_FACT_SEARCH_{uuid.uuid4().hex[:8]}",
        "domain": "workflow",
        "level": 1,
    })
    check("T3", "add_hierarchical_fact basic", r.get("status") == "success",
          f"fact_id={r.get('fact_id', 'NONE')}")

    # Multilingual
    r2 = call_tool("rlm_add_hierarchical_fact", {
        "content": "Тестовый факт на русском языке для проверки",
        "domain": "test-ru",
        "level": 1,
    })
    check("T4", "add_hierarchical_fact multilingual", r2.get("status") == "success",
          f"fact_id={r2.get('fact_id', 'NONE')}")
    return r.get("fact_id"), r2.get("fact_id")


def test_04_search_existing(content_hint: str):
    """Test 5-6: search for existing facts."""
    r = call_tool("rlm_search_facts", {"query": content_hint, "top_k": 5})
    facts = r.get("facts", r.get("results", []))
    check("T5", "search_facts existing query", len(facts) > 0,
          f"found {len(facts)} facts")

    r2 = call_tool("rlm_search_facts", {"query": "Тестовый факт на русском", "top_k": 5})
    facts2 = r2.get("facts", r2.get("results", []))
    check("T6", "search_facts Russian query", len(facts2) > 0,
          f"found {len(facts2)} facts")


def test_05_search_garbage():
    """Test 7 (#34): garbage query returns empty."""
    r = call_tool("rlm_search_facts", {
        "query": "NONEXISTENT_FACT_ABCDEF_ZZZZ_NO_MATCH_XYZ_QQQQQ",
        "top_k": 5,
    })
    facts = r.get("facts", r.get("results", []))
    check("34", "search_facts garbage query returns empty", len(facts) == 0,
          f"got {len(facts)} results (expected 0)")


def test_06_delete_fact(fact_id: str):
    """Test 8-9: delete and verify."""
    r = call_tool("rlm_delete_fact", {"fact_id": fact_id})
    check("T8", "delete_fact", r.get("status") == "success",
          f"status={r.get('status')}")

    # Verify absent from search
    r2 = call_tool("rlm_search_facts", {"query": fact_id, "top_k": 5})
    facts = r2.get("facts", [])
    found = any(fact_id in f.get("id", "") for f in facts)
    check("T9", "deleted fact absent from search", not found,
          f"found={found}")


def test_07_enterprise_context():
    """Test 10-12: enterprise_context."""
    r = call_tool("rlm_enterprise_context", {"query": "test", "include_causal": True})
    check("T10", "enterprise_context include_causal=true", r.get("status") != "error",
          f"keys={list(r.keys())[:5]}")

    r2 = call_tool("rlm_enterprise_context", {"query": "test", "include_causal": False})
    check("T11", "enterprise_context include_causal=false", r2.get("status") != "error",
          f"keys={list(r2.keys())[:5]}")

    # Check no noise
    context_str = json.dumps(r)
    has_fingerprint = "__FINGERPRINT__" in context_str
    has_unknown = "Unknown project" in context_str
    check("T12", "enterprise_context no noise", not has_fingerprint and not has_unknown,
          f"fingerprint={has_fingerprint}, unknown={has_unknown}")


def test_08_causal_decision():
    """Test 13: record_causal_decision."""
    r = call_tool("rlm_record_causal_decision", {
        "decision": "Use monkey-patches instead of forking upstream",
        "reasons": ["Simpler maintenance", "No upstream PR needed"],
    })
    check("T13", "record_causal_decision", "decision_id" in r or r.get("status") == "success",
          f"keys={list(r.keys())[:5]}")


def test_09_route_context():
    """Test 14-15 (#33): route_context no noise."""
    r = call_tool("rlm_route_context", {"query": "test workflow facts"})
    context_str = json.dumps(r)
    has_fingerprint = "__FINGERPRINT__" in context_str
    has_unknown = "is a Unknown project" in context_str
    check("33a", "route_context no fingerprint/unknown", not has_fingerprint and not has_unknown,
          f"fingerprint={has_fingerprint}, unknown={has_unknown}")

    # Check returns actual content
    check("T15", "route_context returns content", len(context_str) > 20,
          f"response length={len(context_str)}")


def test_10_discover_project():
    """Test 16-17 (#32): discover_project."""
    r = call_tool("rlm_discover_project", {"project_root": r"d:\Repos\BIT"})
    project_name = r.get("project_name", "")
    check("32a", "discover_project project_name != 'data'", project_name != "data",
          f"project_name={project_name}")

    warnings = r.get("warnings", [])
    has_guidance = any("mount" in w.lower() or "bind" in w.lower() for w in warnings)
    check("32b", "discover_project has bind-mount guidance", has_guidance,
          f"warnings={warnings[:2]}")


def test_11_ttl_zero():
    """#31: ttl_days=0 creates immediately stale fact."""
    r = call_tool("rlm_add_hierarchical_fact", {
        "content": f"TTL_ZERO_TEST_{uuid.uuid4().hex[:8]}",
        "domain": "test-ttl",
        "level": 1,
        "ttl_days": 0,
    })
    check("31a", "add fact with ttl_days=0", r.get("status") == "success",
          f"fact_id={r.get('fact_id', 'NONE')}")

    # Small delay for TTL processing
    time.sleep(1)

    r2 = call_tool("rlm_get_stale_facts", {})
    stale_count = r2.get("stale_count", 0)
    stale_facts = r2.get("stale_facts", [])
    has_ttl_fact = any("TTL_ZERO_TEST" in f.get("content", "") for f in stale_facts)
    check("31b", "ttl_days=0 fact appears in stale", has_ttl_fact,
          f"stale_count={stale_count}, found_ttl_fact={has_ttl_fact}")


def test_12_get_facts_by_domain():
    """#29 extended: actually call rlm_get_facts_by_domain."""
    r = call_tool("rlm_get_facts_by_domain", {"domain": "workflow"})
    check("29b", "get_facts_by_domain returns results", r.get("status") == "success",
          f"facts_count={r.get('facts_count', 0)}")


def main():
    print("=" * 60)
    print("RLM Regression Test Matrix (#35)")
    print(f"Target: {BASE_URL}")
    print("=" * 60)

    # Initialize MCP session
    try:
        mcp_initialize()
        tools = list_tools()
        print(f"\nServer OK: {len(tools)} tools available\n")
    except Exception as e:
        print(f"\nERROR: Cannot reach server: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)

    print("--- Session & Basic Operations ---")
    test_02_start_session()

    print("\n--- #29: rlm_get_facts_by_domain ---")
    test_01_list_tools()
    test_12_get_facts_by_domain()

    print("\n--- Facts CRUD ---")
    fact_id_en, fact_id_ru = test_03_add_fact()

    print("\n--- #34: search_facts min_score ---")
    test_04_search_existing("TEST_FACT_SEARCH")
    test_05_search_garbage()

    print("\n--- Delete ---")
    if fact_id_en:
        test_06_delete_fact(fact_id_en)

    print("\n--- Enterprise Context ---")
    test_07_enterprise_context()

    print("\n--- Causal Decisions ---")
    test_08_causal_decision()

    print("\n--- #33: route_context noise ---")
    test_09_route_context()

    print("\n--- #32: discover_project ---")
    test_10_discover_project()

    print("\n--- #31: ttl_days=0 ---")
    test_11_ttl_zero()

    print("\n" + "=" * 60)
    print(f"RESULTS: {PASS} PASS, {FAIL} FAIL out of {PASS + FAIL} tests")
    print("=" * 60)

    sys.exit(1 if FAIL > 0 else 0)


if __name__ == "__main__":
    main()
