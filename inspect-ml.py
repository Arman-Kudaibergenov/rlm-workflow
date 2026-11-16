#!/usr/bin/env python3
import os
os.environ.pop("HF_HUB_OFFLINE", None)
import json
from sentence_transformers import SentenceTransformer
name = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
print("downloading/loading", name)
m = SentenceTransformer(name)
print("dim:", m.get_sentence_embedding_dimension())
print("max_seq_length:", m.max_seq_length)
# inspect the transformer config + modules
import glob
base = os.path.expanduser("~/.cache/huggingface/hub/models--sentence-transformers--paraphrase-multilingual-MiniLM-L12-v2")
snaps = glob.glob(base + "/snapshots/*/")
snap = snaps[0]
print("snapshot:", snap)
cfg = json.load(open(snap + "/config.json"))
print("model_type:", cfg.get("model_type"), "hidden:", cfg.get("hidden_size"), "layers:", cfg.get("num_hidden_layers"), "heads:", cfg.get("num_attention_heads"), "vocab:", cfg.get("vocab_size"))
print("modules:", json.load(open(snap + "/modules.json")))
tok = json.load(open(snap + "/tokenizer.json"))
print("tokenizer model type:", tok.get("model", {}).get("type"))
print("normalizer:", tok.get("normalizer"))
# quick semantic probe: Russian query vs Russian fact
import numpy as np
q = m.encode("незавершённые задачи авторизация")
f1 = m.encode("Незавершённые задачи: исправить авторизацию и проверить журнал")
f2 = m.encode("PostgreSQL connection pool limit is 50")
def cos(a,b): return float(np.dot(a,b)/(np.linalg.norm(a)*np.linalg.norm(b)))
print("cos(ru_query, ru_fact)=", round(cos(q,f1),4), "cos(ru_query, en_fact)=", round(cos(q,f2),4))
print("INSPECT_ML_DONE")