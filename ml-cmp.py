#!/usr/bin/env python3
import os, json, math, subprocess
d = json.load(open("/tmp/ml_ref.json"))
MODEL = d["model"]
def cos(a,b):
    s=sum(x*y for x,y in zip(a,b)); na=math.sqrt(sum(x*x for x in a)); nb=math.sqrt(sum(x*x for x in b))
    return s/(na*nb) if na and nb else 0.0
env=dict(os.environ, RUST_LOG="error", RLM_EMBEDDING_MODEL=MODEL, HF_HUB_OFFLINE="1")
worst=1.0
for i,t in enumerate(d["texts"]):
    out=subprocess.check_output(["/opt/rlm-rs/target/release/rlm-rs","--embed-text",t],env=env)
    rs=json.loads(out)
    c=cos(d["ref"][t], rs); worst=min(worst,c)
    print(f"[{i}] cos={c:.6f} dims=py{len(d['ref'][t])}/rs{len(rs)} {t[:45]!r}")
print(f"WORST={worst:.6f} -> {'PASS' if worst>=0.99 else 'FAIL'}")