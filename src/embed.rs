//! Embedder: local candle BERT (default), Ollama HTTP, OpenAI-compatible HTTP.
//! Mirrors _create_embedder() from start_server.py.

use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use tokenizers::Tokenizer;

use crate::bert::BertModel;

pub enum Embedder {
    Local {
        name: String,
        model: BertModel,
        tokenizer: Mutex<Tokenizer>,
        dim: usize,
    },
    Ollama {
        name: String,
        base_url: String,
        client: reqwest::blocking::Client,
        dim: Mutex<usize>,
    },
    OpenAI {
        name: String,
        base_url: String,
        api_key: String,
        client: reqwest::blocking::Client,
        dim: Mutex<usize>,
    },
}

/// Resolve the HF cache snapshot dir for a sentence-transformers model.
pub fn resolve_model_dir(model_name: &str) -> Result<String> {
    if let Ok(dir) = std::env::var("RLM_MODEL_DIR") {
        if !dir.is_empty() {
            return Ok(dir);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let repo = if model_name.contains('/') {
        model_name.to_string()
    } else {
        format!("sentence-transformers/{model_name}")
    };
    let cache_name = format!("models--{}", repo.replace('/', "--"));
    let base = format!("{home}/.cache/huggingface/hub/{cache_name}");
    let ref_path = format!("{base}/refs/main");
    let snapshot = if let Ok(r) = std::fs::read_to_string(&ref_path) {
        r.trim().to_string()
    } else {
        // pick the first snapshot dir
        let mut found = None;
        if let Ok(entries) = std::fs::read_dir(format!("{base}/snapshots")) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    found = Some(e.file_name().to_string_lossy().into_owned());
                    break;
                }
            }
        }
        found.context(format!("no HF snapshot for {model_name} in {base}"))?
    };
    Ok(format!("{base}/snapshots/{snapshot}"))
}

impl Embedder {
    pub fn create(model_name: &str) -> Result<Embedder> {
        let provider = std::env::var("RLM_EMBEDDING_PROVIDER").unwrap_or_default().to_lowercase();
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        if provider == "ollama" {
            let base_url = std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
            let e = Embedder::Ollama {
                name: model_name.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                client,
                dim: Mutex::new(0),
            };
            let dim = e.dimension()?;
            tracing::info!("Embedding: {model_name} via Ollama (dim={dim})");
            return Ok(e);
        }
        if provider == "openai" {
            let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
            let base_url = std::env::var("OPENAI_API_BASE").unwrap_or_default();
            if api_key.is_empty() && base_url.is_empty() {
                bail!("RLM_EMBEDDING_PROVIDER=openai but OPENAI_API_KEY not set");
            }
            let effective = if model_name.is_empty() { "text-embedding-3-small".to_string() } else { model_name.to_string() };
            let base = if base_url.is_empty() { "https://api.openai.com/v1".to_string() } else { base_url.trim_end_matches('/').to_string() };
            let known = match effective.as_str() {
                "text-embedding-3-small" | "text-embedding-ada-002" => 1536,
                "text-embedding-3-large" => 3072,
                _ => 0,
            };
            let e = Embedder::OpenAI {
                name: effective.clone(),
                base_url: base,
                api_key,
                client,
                dim: Mutex::new(known),
            };
            let dim = e.dimension()?;
            tracing::info!("Embedding: {effective} via OpenAI-compatible (dim={dim})");
            return Ok(e);
        }

        // Default: local sentence-transformers model via candle
        let dir = resolve_model_dir(model_name)?;
        let model = BertModel::load(&dir)?;
        let mut tokenizer = Tokenizer::from_file(format!("{dir}/tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer load: {e}"))?;
        // sentence_bert_config.json max_seq_length = 256 overrides tokenizer.json truncation
        let max_len = std::fs::read_to_string(format!("{dir}/sentence_bert_config.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("max_seq_length").and_then(|x| x.as_u64()))
            .unwrap_or(256) as usize;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams { max_length: max_len, ..Default::default() }))
            .map_err(|e| anyhow::anyhow!("truncation: {e}"))?;
        tokenizer.with_padding(None);
        let cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/config.json"))?)?;
        let dim = cfg.get("hidden_size").and_then(|v| v.as_u64()).unwrap_or(384) as usize;
        tracing::info!("Embedding: {model_name} (dim={dim}, local candle)");
        Ok(Embedder::Local { name: model_name.to_string(), model, tokenizer: Mutex::new(tokenizer), dim })
    }

    pub fn model_name(&self) -> &str {
        match self {
            Embedder::Local { name, .. } | Embedder::Ollama { name, .. } | Embedder::OpenAI { name, .. } => name,
        }
    }

    pub fn dimension(&self) -> Result<usize> {
        match self {
            Embedder::Local { dim, .. } => Ok(*dim),
            Embedder::Ollama { dim, .. } | Embedder::OpenAI { dim, .. } => {
                let mut d = dim.lock().unwrap();
                if *d == 0 {
                    let v = self.encode("dimension probe")?;
                    *d = v.len();
                }
                Ok(*d)
            }
        }
    }

    pub fn encode(&self, text: &str) -> Result<Vec<f32>> {
        match self {
            Embedder::Local { model, tokenizer, .. } => {
                let enc = tokenizer
                    .lock()
                    .unwrap()
                    .encode(text, true)
                    .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
                let ids = enc.get_ids().to_vec();
                let mask = enc.get_attention_mask().to_vec();
                let type_ids = enc.get_type_ids().to_vec();
                if ids.is_empty() {
                    bail!("empty tokenization");
                }
                let hidden = model.forward(&ids, &mask, &type_ids)?; // (1, s, h)
                let hidden = hidden.squeeze(0)?; // (s, h)
                let h = hidden.dim(1)?;
                let s = ids.len();
                let mask_f: Vec<f32> = mask.iter().map(|&m| m as f32).collect();
                let data = hidden.to_vec2::<f32>()?;
                let mut sum = vec![0f32; h];
                let mut count = 0f32;
                for (i, row) in data.iter().enumerate().take(s) {
                    let m = mask_f[i];
                    if m > 0.0 {
                        count += m;
                        for j in 0..h {
                            sum[j] += row[j] * m;
                        }
                    }
                }
                if count <= 0.0 {
                    count = 1e-9;
                }
                let mut mean: Vec<f32> = sum.iter().map(|v| v / count).collect();
                // Normalize module (modules.json idx 2)
                let norm: f32 = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for v in mean.iter_mut() {
                        *v /= norm;
                    }
                }
                Ok(mean)
            }
            Embedder::Ollama { base_url, client, name, .. } => {
                let resp: serde_json::Value = client
                    .post(format!("{base_url}/api/embed"))
                    .json(&serde_json::json!({"model": name, "input": [text]}))
                    .send()?
                    .error_for_status()?
                    .json()?;
                let emb = resp
                    .get("embeddings")
                    .and_then(|e| e.get(0))
                    .and_then(|e| e.as_array())
                    .context("ollama: no embeddings in response")?;
                Ok(emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
            }
            Embedder::OpenAI { base_url, api_key, client, name, .. } => {
                let mut req = client
                    .post(format!("{base_url}/embeddings"))
                    .json(&serde_json::json!({"model": name, "input": text}));
                if !api_key.is_empty() {
                    req = req.bearer_auth(api_key);
                }
                let resp: serde_json::Value = req.send()?.error_for_status()?.json()?;
                let emb = resp
                    .get("data")
                    .and_then(|d| d.get(0))
                    .and_then(|d| d.get("embedding"))
                    .and_then(|e| e.as_array())
                    .context("openai: no embedding in response")?;
                Ok(emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
            }
        }
    }
}