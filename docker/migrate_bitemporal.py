#!/usr/bin/env python3
"""One-off migration: bi-temporal validity windows (issue #45).

Model: every fact has valid_from (since when true) and valid_until (when it
stopped being true; NULL = currently valid). stale/archive no longer mean
'hidden by flag' — they mean 'window closed'.

This migration backfills EXISTING rows:
  - valid_from  = created_at where NULL (defensive; schema says NOT NULL
    and 2026-08-02 check showed valid_from == created_at on all rows)
  - valid_until = created_at for rows already marked is_stale/is_archived
    with valid_until IS NULL. ROUGH BUT HONEST: the real close time was
    never recorded (mark_stale/archive_fact only started stamping
    valid_until after #42, and no row has it yet), so we use creation
    time as a lower bound and document the imprecision.
  - live rows keep valid_until = NULL.

New closes (mark_stale / archive_fact / invalidate_fact / soft delete_fact)
stamp valid_until = now at the moment of closing — see #45 in start_server.py.

Run:
    cp /data/.rlm/memory/memory_bridge_v2.db /data/.rlm/memory/memory_bridge_v2.db.bak-bitemporal-20260802
    /opt/rlm/.venv/bin/python /opt/rlm/docker/migrate_bitemporal.py
"""

import sqlite3

DB_PATH = "/data/.rlm/memory/memory_bridge_v2.db"


def main() -> None:
    with sqlite3.connect(DB_PATH) as conn:
        before = conn.execute(
            "SELECT COUNT(*), SUM(valid_until IS NOT NULL) FROM hierarchical_facts"
        ).fetchone()
        print(f"before: total={before[0]} valid_until set={before[1]}")

        cur = conn.execute(
            "UPDATE hierarchical_facts SET valid_from = created_at WHERE valid_from IS NULL"
        )
        print(f"valid_from backfilled: {cur.rowcount}")

        cur = conn.execute(
            "UPDATE hierarchical_facts SET valid_until = created_at "
            "WHERE (is_stale = 1 OR is_archived = 1) AND valid_until IS NULL"
        )
        print(f"valid_until backfilled (rough: = created_at): {cur.rowcount}")

        after = conn.execute(
            "SELECT "
            " SUM(is_stale = 0 AND is_archived = 0 AND valid_until IS NULL),"
            " SUM(valid_until IS NOT NULL) "
            "FROM hierarchical_facts"
        ).fetchone()
        print(f"after: live(open window)={after[0]} closed={after[1]}")

        # sanity: no flagged row left with open window
        bad = conn.execute(
            "SELECT COUNT(*) FROM hierarchical_facts "
            "WHERE (is_stale = 1 OR is_archived = 1) AND valid_until IS NULL"
        ).fetchone()[0]
        if bad:
            raise SystemExit(f"ABORT: {bad} flagged rows still have open window")
        print("sanity OK: every stale/archived row has a closed window")


if __name__ == "__main__":
    main()
