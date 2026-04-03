"""Regression test matrix for GitHub issues #40, #41.

Tests:
- #40: fact_id consistency — get_facts_by_domain and get_stale_facts return fact_id
- #41: enterprise_context description — no 'Zero configuration' in tool description

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
        "clientInfo": {"name": "regression-test-40", "version": "1.0"},
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


def list_tools() -> list:
    """List all registered MCP tools."""
    resp = mcp_call("tools/list")
    result = resp.get("result", {})
    return result.get("tools", [])


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
# #40: fact_id consistency
# ============================================================

def test_40_get_facts_by_domain_fact_id():
    """#40: get_facts_by_domain must return fact_id in each fact."""
    domain = f"test40_{uuid.uuid4().hex[:8]}"

    # Add a fact to the domain
    r = call_tool("rlm_add_hierarchical_fact", {
        "content": f"Test fact for domain {domain}",
        "domain": domain,
        "level": 1,
    })
    check("40a", "add fact to test domain", r.get("status") == "success",
          f"fact_id={r.get('fact_id', 'NONE')}")
    added_fact_id = r.get("fact_id")

    # Get facts by domain
    r2 = call_tool("rlm_get_facts_by_domain", {"domain": domain})
    check("40b", "get_facts_by_domain succeeds", r2.get("status") == "success",
          f"facts_count={r2.get('facts_count', 0)}")

    facts = r2.get("facts", [])
    check("40c", "facts list not empty", len(facts) > 0, f"count={len(facts)}")

    # Verify ALL facts have both id and fact_id (not just first)
    all_have_fact_id = all("fact_id" in f for f in facts) if facts else False
    all_have_id = all("id" in f for f in facts) if facts else False
    check("40d", "ALL facts have fact_id key", all_have_fact_id,
          f"sample_keys={list(facts[0].keys()) if facts else []}")
    check("40e", "backward compat: ALL facts also have id key", all_have_id,
          f"sample_keys={list(facts[0].keys()) if facts else []}")

    # Verify our specific fact is present by matching fact_id
    if facts and added_fact_id:
        our_fact = [f for f in facts if f.get("fact_id") == added_fact_id]
        check("40f_extra", "our added fact found by fact_id", len(our_fact) > 0,
              f"added={added_fact_id}, found={len(our_fact)}")
        if our_fact:
            check("40g_extra", "fact_id == id for our fact",
                  our_fact[0]["fact_id"] == our_fact[0].get("id"),
                  f"fact_id={our_fact[0]['fact_id']}, id={our_fact[0].get('id')}")

    # Cleanup
    if added_fact_id:
        call_tool("rlm_delete_fact", {"fact_id": added_fact_id})


def test_40_get_stale_facts_fact_id():
    """#40: get_stale_facts must return fact_id in each stale fact."""
    # Add a fact with ttl_days=0 (expires immediately → stale)
    marker = f"STALE40_{uuid.uuid4().hex[:8]}"
    r = call_tool("rlm_add_hierarchical_fact", {
        "content": f"Stale test: {marker}",
        "domain": "test40_stale",
        "level": 1,
        "ttl_days": 0,
    })
    check("40f", "add fact with ttl_days=0", r.get("status") == "success",
          f"fact_id={r.get('fact_id', 'NONE')}")
    fact_id = r.get("fact_id")

    # Wait a moment for TTL to process
    time.sleep(1)

    # Get stale facts (this triggers process_expired internally)
    r2 = call_tool("rlm_get_stale_facts", {"include_archived": False})
    check("40g", "get_stale_facts succeeds", r2.get("status") == "success",
          f"stale_count={r2.get('stale_count', 0)}")

    stale = r2.get("stale_facts", [])
    # Note: stale may be empty if TTL hasn't expired yet — check is best-effort
    all_stale_fact_id = all("fact_id" in f for f in stale) if stale else False
    all_stale_id = all("id" in f for f in stale) if stale else False
    check("40h", "stale facts returned", len(stale) > 0,
          f"count={len(stale)}")
    check("40i", "ALL stale facts have fact_id key", all_stale_fact_id or len(stale) == 0,
          f"count={len(stale)}, sample_keys={list(stale[0].keys()) if stale else []}")
    check("40j", "backward compat: ALL stale facts have id key", all_stale_id or len(stale) == 0,
          f"count={len(stale)}, sample_keys={list(stale[0].keys()) if stale else []}")

    # Cleanup
    if fact_id:
        call_tool("rlm_delete_fact", {"fact_id": fact_id})


# ============================================================
# #41: enterprise_context description
# ============================================================

def test_41_enterprise_context_description():
    """#41: enterprise_context description must not say 'Zero configuration'."""
    tools = list_tools()

    ec_tools = [t for t in tools if t.get("name") == "rlm_enterprise_context"]
    check("41a", "rlm_enterprise_context found in tool list", len(ec_tools) > 0,
          f"found {len(ec_tools)} matches")

    if ec_tools:
        desc = ec_tools[0].get("description", "")
        no_zero = "Zero configuration" not in desc
        has_query = "query" in desc.lower() or "requires" in desc.lower()
        check("41b", "description does NOT contain 'Zero configuration'", no_zero,
              f"description={desc[:100]}")
        check("41c", "description mentions query requirement", has_query,
              f"description={desc[:100]}")


def test_41_enterprise_context_with_query():
    """#41: enterprise_context works when called with query param."""
    r = call_tool("rlm_enterprise_context", {
        "query": "test project status",
        "include_causal": False,
    })
    check("41d", "enterprise_context with query succeeds",
          r.get("status") == "success",
          f"status={r.get('status')}, keys={list(r.keys())}")


# ============================================================
# Main
# ============================================================

def main():
    print("=" * 60)
    print("Regression tests for issues #40, #41")
    print(f"Target: {BASE_URL}")
    print("=" * 60)

    # Initialize
    print("\n[init] Initializing MCP session...")
    mcp_initialize()

    # Start session
    r = call_tool("rlm_start_session", {"restore": False})
    print(f"[init] Session: {r.get('status', 'unknown')}")

    # Run tests
    print("\n--- #40: fact_id consistency ---")
    test_40_get_facts_by_domain_fact_id()
    test_40_get_stale_facts_fact_id()

    print("\n--- #41: enterprise_context description ---")
    test_41_enterprise_context_description()
    test_41_enterprise_context_with_query()

    # Summary
    print(f"\n{'=' * 60}")
    print(f"Results: {PASS} PASS, {FAIL} FAIL ({PASS + FAIL} total)")
    print(f"{'=' * 60}")

    return 1 if FAIL > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
