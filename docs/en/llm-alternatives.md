# LLM Backend Options

RLM-Toolkit uses LLMs for semantic search (embeddings) and optionally for
context summarization. You don't need a powerful GPU to run it.

## Embedding Providers

### Option 1: FastEmbed (Zero-config, Recommended for most users)

Built into RLM-Toolkit. Runs on CPU, no API key needed.

```env
RLM_EMBEDDING_PROVIDER=fastembed
# No other config needed
```

The original RLM-Toolkit defaults to `all-MiniLM-L6-v2` (~80MB) — fast and lightweight, but **English only**.

Our infrastructure uses `qwen3-embedding:8b` via Ollama on a dedicated GPU server (see "Our Infrastructure" section below).

The Docker image defaults to `paraphrase-multilingual-MiniLM-L12-v2` (~471MB) — a multilingual model (50+ languages including Russian, Chinese, Arabic), runs on CPU without GPU. You can always replace it with any model you prefer via the `RLM_EMBEDDING_MODEL` variable in `.env`.

**Use this if**: you want zero setup, don't have a GPU, don't want API costs.

---

### Option 2: OpenAI API (Cloud, no local GPU)

Cheapest option if you already have an OpenAI account.
`text-embedding-3-small` costs ~$0.02 per 1M tokens — essentially free for personal use.

```env
RLM_EMBEDDING_PROVIDER=openai
OPENAI_API_KEY=sk-...
RLM_EMBEDDING_MODEL=text-embedding-3-small
```

**Use this if**: you use OpenAI anyway, want cloud convenience, no local hardware.

---

### Option 3: Ollama (Local, CPU or GPU)

[Ollama](https://ollama.com) runs models locally. Works on CPU but faster with GPU.

```bash
# Install Ollama
curl -fsSL https://ollama.com/install.sh | sh

# Pull embedding model (runs on CPU, ~274MB)
ollama pull nomic-embed-text
```

```env
RLM_EMBEDDING_PROVIDER=ollama
OLLAMA_BASE_URL=http://localhost:11434
RLM_EMBEDDING_MODEL=nomic-embed-text
```

**Use this if**: you want full local setup without cloud dependencies.

---

## What About the LLM Itself?

RLM-Toolkit primarily needs embeddings, not a full generative LLM. However,
some features (context summarization, query routing) can use a generative model.

### Replacing our Ollama + qwen3:8b setup

Our infrastructure uses AMD RX 7700 XT with Vulkan for Ollama. For users without
a dedicated GPU:

| Setup | Model | Hardware | Quality |
|-------|-------|----------|---------|
| CPU-only | `qwen2.5:0.5b` via Ollama | Any modern CPU | Minimal |
| Integrated GPU | `phi3.5-mini` via Ollama | Intel/AMD iGPU | Good |
| Discrete GPU 4GB | `llama3.2:3b` via Ollama | GTX 1650+ | Good |
| Discrete GPU 8GB+ | `qwen3:8b` via Ollama | RTX 3060+ / RX 6700+ | Our setup |
| Cloud (no GPU) | `gpt-4o-mini` via OpenAI | None | Excellent |

For most users, **FastEmbed + gpt-4o-mini** is the best balance:
- FastEmbed handles local search (free, private)
- GPT-4o-mini handles complex reasoning tasks (cheap, reliable)

---

## Our Infrastructure (for reference)

For those curious about our specific setup:

- **RLM server**: Proxmox LXC container (CT105), 2 vCPU, 4GB RAM
- **Ollama server**: Proxmox LXC container (CT106), AMD RX 7700 XT via Vulkan
- **Models in use**: `qwen3:8b` (inference), `qwen3-embedding:8b` (embeddings)
- **GPU passthrough**: `OLLAMA_VULKAN=true` with udev rules on Proxmox host

This is significant overkill for RLM alone. FastEmbed or OpenAI API covers 95% of use cases.
