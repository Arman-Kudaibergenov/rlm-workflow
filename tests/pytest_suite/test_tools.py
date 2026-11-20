"""MCP tool-level tests for the isolated pytest suite."""

from __future__ import annotations

import json
import subprocess
import uuid


def test_start_session_restore_false(client):
    result = client.call("rlm_start_session", restore=False)
    assert result.get("restored") is False


def test_start_session_restore_true(client):
    marker = f"RESTORE_{uuid.uuid4().hex[:8]}"
    client.call("rlm_start_session", restore=False)
    created = client.call(
        "rlm_add_hierarchical_fact",
        content=f"Restore checkpoint {marker}",
        domain="workflow",
        level=1,
    )
    fact_id = created["fact_id"]
    client.call("rlm_sync_state")

    restored_client = client.__class__(base_url=client.base_url)
    restored_client.initialize()
    result = restored_client.call("rlm_start_session", restore=True)
    assert result.get("restored") is True

    search = restored_client.call(
        "rlm_search_facts",
        query=marker,
        keyword_weight=0.9,
        recency_weight=0.0,
        top_k=5,
    )
    found_ids = [item["fact_id"] for item in search.get("results", [])]
    assert fact_id in found_ids
    restored_client.call("rlm_delete_fact", fact_id=fact_id)


def test_add_fact_returns_fact_id(rlm):
    result = rlm.call(
        "rlm_add_hierarchical_fact",
        content="Test fact for pytest suite",
        domain="workflow",
        level=1,
    )
    assert result.get("status") == "success"
    assert result.get("fact_id")
    rlm._created_ids.append(result["fact_id"])


def test_search_existing_fact(rlm, fact_seed):
    result = rlm.call(
        "rlm_search_facts",
        query=fact_seed,
        keyword_weight=0.9,
        semantic_weight=0.1,
        recency_weight=0.0,
        top_k=5,
    )
    matches = result.get("results", [])
    assert matches, "Expected results for the seeded marker"
    assert any(fact_seed in item.get("content", "") for item in matches)


def test_search_nonexistent_returns_empty(rlm):
    result = rlm.call(
        "rlm_search_facts",
        query="xyzzy12345nonsense_pytest_marker_impossible",
        keyword_weight=0.9,
        semantic_weight=0.1,
        recency_weight=0.0,
        top_k=5,
    )
    assert result.get("results") == []


def test_delete_fact_removes_search_hit(rlm):
    marker = f"DELETE_ME_{uuid.uuid4().hex[:8]}"
    created = rlm.call(
        "rlm_add_hierarchical_fact",
        content=f"Unique fact {marker}",
        domain="workflow",
        level=1,
    )
    fact_id = created["fact_id"]
    rlm.call("rlm_delete_fact", fact_id=fact_id)

    result = rlm.call(
        "rlm_search_facts",
        query=marker,
        keyword_weight=0.9,
        recency_weight=0.0,
        top_k=10,
    )
    found_ids = [item["fact_id"] for item in result.get("results", [])]
    assert fact_id not in found_ids


def test_enterprise_context_include_causal(rlm, causal_seed):
    result = rlm.call(
        "rlm_enterprise_context",
        query=causal_seed,
        include_causal=True,
    )
    assert result.get("causal_included") is True
    result_str = json.dumps(result, ensure_ascii=False)
    assert causal_seed in result_str


def test_enterprise_context_no_noise(rlm):
    result = rlm.call("rlm_enterprise_context", query="test", include_causal=False)
    result_str = json.dumps(result, ensure_ascii=False)
    assert "__FINGERPRINT__" not in result_str
    assert "Unknown project" not in result_str


def test_record_causal_decision(rlm):
    result = rlm.call(
        "rlm_record_causal_decision",
        decision="Test architecture decision",
        reasons=["For testing purposes"],
        alternatives=["none"],
        consequences=["Test only, no real impact"],
    )
    assert result.get("decision_id") or result.get("status") == "success"


def test_route_context_returns_facts(rlm, fact_seed):
    result = rlm.call("rlm_route_context", query=f"PENDING tasks {fact_seed}")
    assert result.get("facts_count", 0) > 0


def test_discover_project_nonexistent_path(rlm):
    result = rlm.call(
        "rlm_discover_project",
        project_root="/nonexistent/path/xyzzy_pytest",
    )
    has_warning = bool(result.get("warnings")) or result.get("status") == "error"
    assert has_warning


def test_no_float32_in_logs(rlm_env):
    assert rlm_env["container_name"], "A Docker container is required for log inspection"
    result = subprocess.run(
        ["docker", "logs", rlm_env["container_name"]],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    logs = result.stdout + result.stderr
    assert "not JSON serializable" not in logs


def test_single_embedding_model_for_new_fact(rlm, rlm_env):
    assert rlm_env["container_name"], "A Docker container is required for DB inspection"
    created = rlm.call(
        "rlm_add_hierarchical_fact",
        content="Embedding model isolation test",
        domain="workflow",
        level=1,
    )
    fact_id = created["fact_id"]
    rlm._created_ids.append(fact_id)

    script = (
        "import json, sqlite3; "
        f"conn = sqlite3.connect({rlm_env['sqlite_path']!r}); "
        f"print(json.dumps(conn.execute(\"SELECT DISTINCT model_name FROM embeddings_index WHERE fact_id = '{fact_id}'\").fetchall(), ensure_ascii=False))"
    )
    result = subprocess.run(
        ["docker", "exec", rlm_env["container_name"], "python3", "-c", script],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    models = [row[0] for row in json.loads(result.stdout.strip())] if result.stdout.strip() else []

    assert models, "Expected at least one embedding row for the new fact"
    assert len(models) == 1
    assert models[0] == rlm_env["expected_model"]
