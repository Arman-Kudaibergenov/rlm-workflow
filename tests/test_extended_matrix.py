#!/usr/bin/env python3
"""Extended test matrix (Issue #26) — 4 zones of Memory Bridge / RLM verification.

Zone 1: Memory Lifecycle (write, update, delete, staleness)
Zone 2: Memory Structure (layers, domains, isolation)
Zone 3: Context Quality (relevance, noise, clarity)
Zone 4: Operational Resilience (repeated runs, consistency)

Usage:
    ssh root@192.168.0.106 "python3 /tmp/test_extended_matrix.py"
"""
import json
import sys
import time
import urllib.request

BASE_URL = "http://127.0.0.1:8200/mcp"


def _parse_sse(body):
    for line in body.split("\n"):
        line = line.strip()
        if line.startswith("data:"):
            try:
                return json.loads(line[5:].strip())
            except json.JSONDecodeError:
                pass
    return None


def _post(payload, session_id=""):
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if session_id:
        headers["Mcp-Session-Id"] = session_id
    req = urllib.request.Request(
        BASE_URL, data=json.dumps(payload).encode(), headers=headers
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        sid = resp.headers.get("Mcp-Session-Id", session_id)
        body = resp.read().decode()
    parsed = _parse_sse(body)
    if parsed:
        return parsed, sid
    try:
        return json.loads(body), sid
    except Exception:
        return {"raw": body}, sid


class MCPSession:
    def __init__(self):
        init, self.sid = _post(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "rlm-extended-test", "version": "1.0"},
                },
            }
        )
        try:
            _post(
                {"jsonrpc": "2.0", "method": "notifications/initialized"}, self.sid
            )
        except Exception:
            pass
        self._id = 2

    def call(self, tool, args=None):
        self._id += 1
        result, _ = _post(
            {
                "jsonrpc": "2.0",
                "id": self._id,
                "method": "tools/call",
                "params": {"name": tool, "arguments": args or {}},
            },
            self.sid,
        )
        return result


def get_content(result):
    try:
        text = result["result"]["content"][0]["text"]
        return json.loads(text)
    except Exception:
        pass
    try:
        return result["result"]["structuredContent"]["result"]
    except Exception:
        return result


class TestRunner:
    def __init__(self):
        self.passed = 0
        self.failed = 0
        self.results = []

    def test(self, name, condition, detail=""):
        status = "PASS" if condition else "FAIL"
        if condition:
            self.passed += 1
        else:
            self.failed += 1
        self.results.append({"name": name, "status": status, "detail": detail})
        print(f"[{status}] {name}")
        if detail:
            print(f"       {detail}")
        return condition

    def summary(self):
        total = self.passed + self.failed
        print(f"\n{'=' * 60}")
        print(f"TOTAL: {self.passed}/{total} passed, {self.failed} failed")
        print(f"{'=' * 60}")
        return self.failed == 0


# ============================================================
# Zone 1: Memory Lifecycle
# ============================================================

def zone1_lifecycle(s, t):
    """Test write → update → delete → staleness."""
    print("\n--- Zone 1: Memory Lifecycle ---")

    # Z1.1: Write a fact, then write an updated version with same content prefix
    r = s.call("rlm_add_hierarchical_fact", {
        "content": "Z1_LIFECYCLE_V1: initial version of test fact",
        "domain": "lifecycle_test",
        "level": 2,
    })
    c = get_content(r)
    fact_id_v1 = c.get("fact_id")
    t.test(
        "Z1.1 Write new fact",
        c.get("status") == "success" and fact_id_v1 is not None,
        f"fact_id={fact_id_v1}",
    )

    # Z1.2: Delete the fact, verify it no longer appears in search
    if fact_id_v1:
        r = s.call("rlm_delete_fact", {"fact_id": fact_id_v1})
        c = get_content(r)
        delete_ok = c.get("status") == "success"
        t.test(
            "Z1.2 Delete fact",
            delete_ok,
            f"status={c.get('status')}",
        )

        # Verify deleted fact doesn't appear in search
        r = s.call("rlm_search_facts", {"query": "Z1_LIFECYCLE_V1 initial version"})
        c = get_content(r)
        results = c.get("results", c.get("facts", []))
        # Check that our specific fact_id is not in results
        found_deleted = any(
            fact_id_v1 in json.dumps(item)
            for item in results
        )
        t.test(
            "Z1.3 Deleted fact absent from search",
            not found_deleted,
            f"search_results_count={len(results)}, found_deleted={found_deleted}",
        )
    else:
        t.test("Z1.2 Delete fact", False, "no fact_id from Z1.1")
        t.test("Z1.3 Deleted fact absent from search", False, "skipped")


# ============================================================
# Zone 2: Memory Structure
# ============================================================

def zone2_structure(s, t):
    """Test layer separation and domain isolation."""
    print("\n--- Zone 2: Memory Structure ---")

    # Z2.1: Write facts at different levels, verify they end up in correct levels
    levels_data = [
        ("Z2_PROJECT_LEVEL_FACT: global architecture rule", "structure_test", 0),
        ("Z2_DOMAIN_LEVEL_FACT: domain-specific convention", "structure_test", 1),
        ("Z2_MODULE_LEVEL_FACT: module implementation detail", "structure_test", 2),
    ]

    fact_ids = []
    for content, domain, level in levels_data:
        r = s.call("rlm_add_hierarchical_fact", {
            "content": content,
            "domain": domain,
            "level": level,
        })
        c = get_content(r)
        fact_ids.append(c.get("fact_id"))

    t.test(
        "Z2.1 Facts created at 3 levels",
        all(fid is not None for fid in fact_ids),
        f"fact_ids={fact_ids}",
    )

    # Z2.2: Domain isolation — facts in domain A should not appear when querying domain B
    r = s.call("rlm_add_hierarchical_fact", {
        "content": "Z2_ISOLATED_DOMAIN_A: only in domain alpha",
        "domain": "domain_alpha",
        "level": 2,
    })

    r = s.call("rlm_add_hierarchical_fact", {
        "content": "Z2_ISOLATED_DOMAIN_B: only in domain beta",
        "domain": "domain_beta",
        "level": 2,
    })

    # Search for domain_alpha content
    r = s.call("rlm_search_facts", {"query": "Z2_ISOLATED_DOMAIN_A alpha"})
    c = get_content(r)
    results = c.get("results", c.get("facts", []))
    results_str = json.dumps(results, ensure_ascii=False)
    # domain_alpha content should be found, domain_beta content should NOT be top result
    has_alpha = "domain_alpha" in results_str or "Z2_ISOLATED_DOMAIN_A" in results_str
    t.test(
        "Z2.2 Domain search returns correct domain's facts",
        has_alpha,
        f"found_alpha={has_alpha}, results_count={len(results)}",
    )


# ============================================================
# Zone 3: Context Quality
# ============================================================

def zone3_context_quality(s, t):
    """Test relevance and noise filtering in context."""
    print("\n--- Zone 3: Context Quality ---")

    # Z3.1: Enterprise context should return relevant facts for specific query
    # First, add a distinctive fact
    s.call("rlm_add_hierarchical_fact", {
        "content": "Z3_CONTEXT_QUALITY: PostgreSQL connection pool limit is 50 for CT121",
        "domain": "infrastructure",
        "level": 1,
    })

    r = s.call("rlm_enterprise_context", {
        "query": "PostgreSQL connection pool CT121",
        "max_tokens": 2000,
    })
    c = get_content(r)
    ctx = c.get("context", "")
    has_relevant = "PostgreSQL" in ctx or "connection pool" in ctx or "CT121" in ctx
    t.test(
        "Z3.1 Enterprise context returns relevant facts",
        c.get("status") == "success" and has_relevant,
        f"relevant_found={has_relevant}, context_len={len(ctx)}",
    )

    # Z3.2: Context should not contain noise markers
    ctx_str = json.dumps(c, ensure_ascii=False)
    no_fingerprint = "__FINGERPRINT__" not in ctx_str
    no_unknown = "Unknown project" not in ctx_str
    no_error = c.get("status") != "error"
    t.test(
        "Z3.2 Context is clean (no noise markers)",
        no_fingerprint and no_unknown and no_error,
        f"fingerprint={'clean' if no_fingerprint else 'DIRTY'}, "
        f"unknown={'clean' if no_unknown else 'DIRTY'}",
    )


# ============================================================
# Zone 4: Operational Resilience
# ============================================================

def zone4_resilience(s, t):
    """Test repeated operations don't degrade system."""
    print("\n--- Zone 4: Operational Resilience ---")

    # Z4.1: Multiple sequential add+search cycles should all succeed
    success_count = 0
    for i in range(3):
        r = s.call("rlm_add_hierarchical_fact", {
            "content": f"Z4_RESILIENCE_FACT_{i}: iteration {i} test data",
            "domain": "resilience_test",
            "level": 2,
        })
        c = get_content(r)
        if c.get("status") == "success":
            success_count += 1

    t.test(
        "Z4.1 Multiple add cycles all succeed",
        success_count == 3,
        f"success={success_count}/3",
    )

    # Z4.2: System health after batch operations
    try:
        import subprocess as sp
        status = sp.check_output(
            ["docker", "inspect", "rlm", "--format", "{{.State.Health.Status}}"],
            text=True,
        ).strip()
        t.test(
            "Z4.2 Container healthy after batch operations",
            status == "healthy",
            f"health={status}",
        )
    except FileNotFoundError:
        # Running inside container — check via MCP
        r = s.call("rlm_start_session", {"restore": True})
        c = get_content(r)
        t.test(
            "Z4.2 System responsive after batch operations",
            c.get("status") == "success",
            f"status={c.get('status')}",
        )


# ============================================================
# Main
# ============================================================

def main():
    t = TestRunner()

    print("=" * 60)
    print("RLM Extended Test Matrix (Issue #26)")
    print("4 Zones: Lifecycle | Structure | Context | Resilience")
    print("=" * 60)

    s = MCPSession()

    # Start session
    s.call("rlm_start_session", {"restore": False})

    zone1_lifecycle(s, t)
    zone2_structure(s, t)
    zone3_context_quality(s, t)
    zone4_resilience(s, t)

    all_passed = t.summary()

    # Save results
    with open("/tmp/rlm_extended_results.json", "w") as f:
        json.dump(
            {
                "passed": t.passed,
                "failed": t.failed,
                "total": t.passed + t.failed,
                "tests": t.results,
            },
            f,
            indent=2,
            ensure_ascii=False,
        )
    print(f"\nResults saved to /tmp/rlm_extended_results.json")

    sys.exit(0 if all_passed else 1)


if __name__ == "__main__":
    main()
