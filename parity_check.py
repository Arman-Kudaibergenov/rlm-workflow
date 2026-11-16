#!/usr/bin/env python3
"""Done-when #4: cosine parity between Python (sentence-transformers) and Rust (candle)
embeddings of the same texts. Run on host rlm with /opt/rlm/.venv/bin/python."""
import json
import math
import os
import subprocess

os.environ["HF_HUB_OFFLINE"] = "1"
from sentence_transformers import SentenceTransformer

model = SentenceTransformer("all-MiniLM-L6-v2")

TEXTS = [
    "The quick brown fox jumps over the lazy dog",
    "The production instance lives on the internal tailnet host behind the gateway",
    "Незавершённые задачи: исправить авторизацию и проверить журнал регистрации",
    "ibcmd extension import exhausted memory on the 1C farm container",
    "domain: infrastructure | level: 1 | title: PostgreSQL farm | <container-id> hosts the 1C bases on PostgreSQL 16 with the mchar extension",
    "x",
    "A" * 1200,
]


def cos(a, b):
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(x * x for x in b))
    return dot / (na * nb) if na and nb else 0.0


env = dict(os.environ, RUST_LOG="error")
worst = 1.0
for i, t in enumerate(TEXTS):
    py = model.encode(t).tolist()
    out = subprocess.check_output(
        ["/opt/rlm-rs/target/release/rlm-rs", "--embed-text", t], env=env
    )
    rs = json.loads(out)
    assert len(py) == len(rs) == 384, f"dim mismatch: py={len(py)} rs={len(rs)}"
    c = cos(py, rs)
    worst = min(worst, c)
    label = t[:60].replace("\n", " ")
    print(f"[{i}] cos={c:.6f}  dims={len(rs)}  text={label!r}")

print(f"\nWORST cosine = {worst:.6f}  (threshold 0.99) -> {'PASS' if worst >= 0.99 else 'FAIL'}")