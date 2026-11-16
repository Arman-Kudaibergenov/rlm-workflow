#!/usr/bin/env python3
import os, json
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
ref = {t: m.encode(t).tolist() for t in TEXTS}
json.dump({"model": MODEL, "texts": TEXTS, "ref": ref}, open("/tmp/ml_ref.json", "w"))
print("REF_SAVED", len(TEXTS))