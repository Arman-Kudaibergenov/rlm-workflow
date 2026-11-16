#!/usr/bin/env python3
import os, json, math, subprocess
os.environ["HF_HUB_OFFLINE"] = "1"
from sentence_transformers import SentenceTransformer
MODEL = "paraphrase-multilingual-MiniLM-L12-v2"
m = SentenceTransformer(MODEL)
TEXTS = [
    "The quick brown fox jumps over the lazy dog",
    "Незавершённые задачи: исправить авторизацию и проверить журнал регистрации",
    "The production instance lives on the internal tailnet host",
    "domain: infrastructure | level: 1 | title: PostgreSQL farm | <container-id> hosts the 1C bases",
]
def cos(a,b):
    d=sum(x*y for x,y in zip(a,b)); na=math.sqrt(sum(x*x for x in a)); nb=math.sqrt(sum(x*x for x in b))
    return d/(na*nb) if na and nb else 0.0
env=dict(os.environ, RUST_LOG="error", RLM_EMBEDDING_MODEL=MODEL)
worst=1.0
for i,t in enumerate(TEXTS):
    py=m.encode(t).tolist()
    out=subprocess.check_output(["/opt/rlm-rs/target/release/rlm-rs","--embed-text",t],env=env)
    rs=json.loads(out)
    c=cos(py,rs); worst=min(worst,c)
    print(f"[{i}] cos={c:.6f} dims=py{len(py)}/rs{len(rs)} {t[:45]!r}")
print(f"WORST={worst:.6f} -> {'PASS' if worst>=0.99 else 'FAIL'}")