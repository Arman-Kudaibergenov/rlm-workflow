"""Scenario tests for session behavior and data hygiene."""

from __future__ import annotations

import uuid


def test_session_continuity(client):
    marker = f"CONTINUITY_{uuid.uuid4().hex[:8]}"
    client._created_ids = []
    client.call("rlm_start_session", restore=False)

    created = client.call(
        "rlm_add_hierarchical_fact",
        content=f"Session continuity test: {marker}",
        domain="workflow",
        level=1,
    )
    fact_id = created["fact_id"]

    client.call("rlm_start_session", restore=True)
    result = client.call(
        "rlm_search_facts",
        query=marker,
        keyword_weight=0.9,
        recency_weight=0.0,
        top_k=5,
    )
    found_ids = [item["fact_id"] for item in result.get("results", [])]

    client.call("rlm_delete_fact", fact_id=fact_id)
    assert fact_id in found_ids


def test_semantic_relevance(rlm, fact_seed):
    result = rlm.call(
        "rlm_search_facts",
        query=f"unfinished tasks for the next session {fact_seed}",
        semantic_weight=0.5,
        keyword_weight=0.4,
        recency_weight=0.1,
        top_k=10,
    )
    assert result.get("results"), "Expected PENDING-style facts in semantic search"
    assert any(fact_seed in item.get("content", "") for item in result["results"])


def test_domain_isolation(rlm):
    marker = f"DOMAIN_ISO_{uuid.uuid4().hex[:8]}"

    workflow_fact = rlm.call(
        "rlm_add_hierarchical_fact",
        content=f"workflow-only {marker}",
        domain="workflow",
        level=1,
    )
    vertical_fact = rlm.call(
        "rlm_add_hierarchical_fact",
        content=f"vertical-only {marker}",
        domain="vertical",
        level=1,
    )
    rlm._created_ids.extend([workflow_fact["fact_id"], vertical_fact["fact_id"]])

    def fact_ids(domain: str) -> set[str]:
        facts = rlm.call("rlm_get_facts_by_domain", domain=domain).get("facts", [])
        return {fact.get("id") or fact.get("fact_id") for fact in facts}

    workflow_ids = fact_ids("workflow")
    vertical_ids = fact_ids("vertical")

    assert workflow_fact["fact_id"] in workflow_ids
    assert workflow_fact["fact_id"] not in vertical_ids
    assert vertical_fact["fact_id"] in vertical_ids
    assert vertical_fact["fact_id"] not in workflow_ids


def test_repeated_runs_stable(rlm, fact_seed):
    params = {
        "query": f"PENDING tasks {fact_seed}",
        "keyword_weight": 0.7,
        "semantic_weight": 0.2,
        "recency_weight": 0.1,
        "top_k": 5,
    }
    runs = [
        [item["fact_id"] for item in rlm.call("rlm_search_facts", **params).get("results", [])]
        for _ in range(3)
    ]
    assert runs[0] == runs[1] == runs[2]


def test_add_delete_cycle_restores_total_facts(rlm):
    baseline = rlm.call("rlm_get_hierarchy_stats")
    baseline_total = baseline["memory_store"]["total_facts"]

    created_ids = []
    for index in range(5):
        created = rlm.call(
            "rlm_add_hierarchical_fact",
            content=f"Cycle test fact {index} ADDDELETE_pytest",
            domain="workflow",
            level=1,
        )
        created_ids.append(created["fact_id"])

    for fact_id in created_ids:
        rlm.call("rlm_delete_fact", fact_id=fact_id)

    after = rlm.call("rlm_get_hierarchy_stats")
    assert after["memory_store"]["total_facts"] == baseline_total


def test_deleted_fact_does_not_become_stale(rlm):
    created = rlm.call(
        "rlm_add_hierarchical_fact",
        content="Stale test STALE_pytest",
        domain="workflow",
        level=3,
    )
    fact_id = created["fact_id"]
    rlm.call("rlm_delete_fact", fact_id=fact_id)

    stale = rlm.call("rlm_get_stale_facts")
    stale_ids = [fact.get("fact_id") or fact.get("id") for fact in stale.get("stale_facts", [])]
    assert fact_id not in stale_ids
