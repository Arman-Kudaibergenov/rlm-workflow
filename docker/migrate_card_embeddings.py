#!/usr/bin/env python3
"""One-off migration: re-embed all live facts with CARD text (issue #43).

Card text = "domain: {domain} | level: {level} | title: {first line} | {content}"
instead of raw content — proven in bsl-atlas to lift hit@5 from 0 to 0.78.

Model is NOT changed: all-minilm on Ollama == all-MiniLM-L6-v2 (dim 384),
the same model the server embeds queries with (RLM_EMBEDDING_MODEL).
embeddings_index.model_name stays 'all-MiniLM-L6-v2'.

KEEP card_text() IN SYNC with _card_text() in start_server.py (#43).

Usage:
    /opt/rlm/.venv/bin/python /opt/rlm/docker/migrate_card_embeddings.py [--dry-run]

Back up the DB first:
    cp /data/.rlm/memory/memory_bridge_v2.db /data/.rlm/memory/memory_bridge_v2.db.bak-cards-20260802
"""

import json
import os
import sqlite3
import sys
import time
import urllib.request
from datetime import datetime

DB_PATH = "/data/.rlm/memory/memory_bridge_v2.db"
OLLAMA_URL = os.environ.get("OLLAMA_BASE_URL", "http://127.0.0.1:11434") + "/api/embed"
MODEL = "all-minilm"
MODEL_NAME = "all-MiniLM-L6-v2"  # recorded in embeddings_index — unchanged model
EXPECTED_DIM = 384
BATCH = 32


def card_text(content: str, domain: str | None, level: int, module: str | None = None) -> str:
    """Structured card wrapper for embedding. KEEP IN SYNC with start_server #43.

    Truncated to 1000 chars: MiniLM sees max 256 wordpieces (~1000 chars) —
    anything beyond is discarded by the model anyway, and Ollama rejects
    over-context inputs with HTTP 400.
    """
    first = content.strip().splitlines()[0][:120] if content.strip() else ""
    parts = [f"domain: {domain or 'general'}", f"level: {level}"]
    if module:
        parts.append(f"module: {module}")
    if first:
        parts.append(f"title: {first}")
    return (" | ".join(parts) + " | " + content)[:1000]


def ollama_embed(texts: list[str]) -> list[list[float]]:
    payload = json.dumps({
        "model": MODEL,
        "input": texts,
        # Ollama defaults all-minilm to num_ctx=128 and 400s on longer input.
        # 512 matches the model's positional cap; card_text is <=1000 chars anyway.
        "options": {"num_ctx": 512},
    }).encode()
    req = urllib.request.Request(
        OLLAMA_URL, data=payload, headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read())
        return data["embeddings"]
    except urllib.error.HTTPError as e:
        if len(texts) == 1:
            # Cyrillic tokenizes ~2-3x worse than ASCII in MiniLM; even <=1000
            # chars can overflow num_ctx. Degrade by truncating, not by failing.
            body = e.read()[:200]
            for cut in (400, 250, 120):
                if len(texts[0]) > cut:
                    print(f"  warn: 400 on {len(texts[0])}-char input ({body!r}), retry at {cut} chars")
                    try:
                        return ollama_embed([texts[0][:cut]])
                    except urllib.error.HTTPError:
                        continue
            raise
        # Isolate the bad input: retry per-item
        out = []
        for t in texts:
            out.extend(ollama_embed([t]))
        return out


def main() -> None:
    dry_run = "--dry-run" in sys.argv

    # Preflight: model alive + dimension matches the runtime query embedder
    probe = ollama_embed(["dimension probe"])
    dim = len(probe[0])
    if dim != EXPECTED_DIM:
        sys.exit(f"ABORT: {MODEL} dim={dim}, expected {EXPECTED_DIM} — would break query/doc compatibility")
    print(f"preflight OK: {MODEL} dim={dim} on {OLLAMA_URL}")

    with sqlite3.connect(DB_PATH) as conn:
        conn.row_factory = sqlite3.Row
        rows = conn.execute(
            "SELECT id, content, domain, module, level FROM hierarchical_facts "
            "WHERE is_archived = 0 ORDER BY created_at"
        ).fetchall()
    print(f"live facts to re-embed: {len(rows)}")
    if dry_run:
        for r in rows[:3]:
            print("---", r["id"])
            print(card_text(r["content"], r["domain"], r["level"], r["module"])[:300])
        return

    t0 = time.time()
    done = 0
    for i in range(0, len(rows), BATCH):
        batch = rows[i : i + BATCH]
        texts = [card_text(r["content"], r["domain"], r["level"], r["module"]) for r in batch]
        vectors = ollama_embed(texts)
        if len(vectors) != len(batch):
            sys.exit(f"ABORT: ollama returned {len(vectors)} vectors for {len(batch)} inputs")
        now = datetime.now().isoformat()
        with sqlite3.connect(DB_PATH) as conn:
            for r, vec in zip(batch, vectors):
                blob = json.dumps(vec)
                conn.execute(
                    "UPDATE hierarchical_facts SET embedding = ? WHERE id = ?",
                    (blob, r["id"]),
                )
                conn.execute(
                    "INSERT OR REPLACE INTO embeddings_index (fact_id, embedding, model_name, updated_at) "
                    "VALUES (?, ?, ?, ?)",
                    (r["id"], blob, MODEL_NAME, now),
                )
        done += len(batch)
        if done % 160 < BATCH or done == len(rows):
            rate = done / max(time.time() - t0, 0.001)
            print(f"  {done}/{len(rows)} ({rate:.1f} facts/s)")

    print(f"migration done: {done} facts in {time.time() - t0:.0f}s, model_name='{MODEL_NAME}' (unchanged)")


if __name__ == "__main__":
    main()
