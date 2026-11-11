//! Minimal BERT inference on candle-nn for all-MiniLM-L6-v2.
//! Matches HuggingFace BertModel forward (mean pooling + L2 normalize done in embed.rs).

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::{linear, Linear, Module, VarBuilder};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct BertConfig {
    pub hidden_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,
    pub intermediate_size: usize,
    pub vocab_size: usize,
    pub max_position_embeddings: usize,
    pub layer_norm_eps: f64,
    #[serde(default = "default_type_vocab")]
    pub type_vocab_size: usize,
}

fn default_type_vocab() -> usize {
    2
}

struct LayerNorm {
    weight: Tensor,
    bias: Tensor,
    eps: f32,
}

impl LayerNorm {
    fn load(vb: VarBuilder, size: usize, eps: f64) -> Result<Self> {
        Ok(LayerNorm {
            weight: vb.get(size, "weight")?,
            bias: vb.get(size, "bias")?,
            eps: eps as f32,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mean = x.mean_keepdim(candle_core::D::Minus1)?;
        let centered = x.broadcast_sub(&mean)?;
        let var = (&centered * &centered)?.mean_keepdim(candle_core::D::Minus1)?;
        let normed = centered.broadcast_div(&(var + self.eps as f64)?.sqrt()?)?;
        Ok(normed.broadcast_mul(&self.weight)?.broadcast_add(&self.bias)?)
    }
}

struct Attention {
    query: Linear,
    key: Linear,
    value: Linear,
    output: Linear,
    out_ln: LayerNorm,
    num_heads: usize,
    head_dim: usize,
}

impl Attention {
    fn load(vb: VarBuilder, cfg: &BertConfig) -> Result<Self> {
        let h = cfg.hidden_size;
        Ok(Attention {
            query: linear(h, h, vb.pp("attention.self.query"))?,
            key: linear(h, h, vb.pp("attention.self.key"))?,
            value: linear(h, h, vb.pp("attention.self.value"))?,
            output: linear(h, h, vb.pp("attention.output.dense"))?,
            out_ln: LayerNorm::load(vb.pp("attention.output.LayerNorm"), h, cfg.layer_norm_eps)?,
            num_heads: cfg.num_attention_heads,
            head_dim: h / cfg.num_attention_heads,
        })
    }

    fn forward(&self, x: &Tensor, mask: &Tensor) -> Result<Tensor> {
        let (b, s, _) = x.dims3()?;
        let nh = self.num_heads;
        let hd = self.head_dim;

        let reshape = |t: Tensor| -> Result<Tensor> {
            Ok(t.reshape((b, s, nh, hd))?.transpose(1, 2)?.contiguous()?)
        };
        let q = reshape(self.query.forward(x)?)?;
        let k = reshape(self.key.forward(x)?)?;
        let v = reshape(self.value.forward(x)?)?;

        let scale = (hd as f64).sqrt();
        let scores = (q.matmul(&k.transpose(2, 3)?)? / scale)?;
        // additive padding mask: (b, 1, 1, s)
        let scores = scores.broadcast_add(mask)?;
        let probs = candle_nn::ops::softmax_last_dim(&scores)?;
        let ctx = probs.matmul(&v)?; // (b, nh, s, hd)
        let ctx = ctx.transpose(1, 2)?.reshape((b, s, nh * hd))?;
        let out = self.output.forward(&ctx)?;
        self.out_ln.forward(&(out + x)?)
    }
}

struct EncoderLayer {
    attention: Attention,
    intermediate: Linear,
    output: Linear,
    out_ln: LayerNorm,
}

impl EncoderLayer {
    fn load(vb: VarBuilder, cfg: &BertConfig) -> Result<Self> {
        let h = cfg.hidden_size;
        Ok(EncoderLayer {
            attention: Attention::load(vb.clone(), cfg)?,
            intermediate: linear(h, cfg.intermediate_size, vb.pp("intermediate.dense"))?,
            output: linear(cfg.intermediate_size, h, vb.pp("output.dense"))?,
            out_ln: LayerNorm::load(vb.pp("output.LayerNorm"), h, cfg.layer_norm_eps)?,
        })
    }

    fn forward(&self, x: &Tensor, mask: &Tensor) -> Result<Tensor> {
        let h = self.attention.forward(x, mask)?;
        let inter = self.intermediate.forward(&h)?;
        // exact GELU (erf), HF hidden_act="gelu"
        let inter = gelu(&inter)?;
        let out = self.output.forward(&inter)?;
        self.out_ln.forward(&(out + h)?)
    }
}

fn gelu(x: &Tensor) -> Result<Tensor> {
    let half = (x * 0.5)?;
    let inner = (x / (2f64).sqrt())?.erf()?;
    let shifted = (inner + 1.0)?;
    Ok((half * shifted)?)
}

pub struct BertModel {
    word_embeddings: Tensor,
    position_embeddings: Tensor,
    token_type_embeddings: Tensor,
    emb_ln: LayerNorm,
    layers: Vec<EncoderLayer>,
    device: Device,
}

impl BertModel {
    pub fn load(model_dir: &str) -> Result<Self> {
        let device = Device::Cpu;
        let cfg_str = std::fs::read_to_string(format!("{model_dir}/config.json"))
            .context("read config.json")?;
        let cfg: BertConfig = serde_json::from_str(&cfg_str).context("parse config.json")?;

        let tensors = candle_core::safetensors::load(format!("{model_dir}/model.safetensors"), &device)
            .context("load model.safetensors")?;
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);

        let h = cfg.hidden_size;
        let word_embeddings = vb.pp("embeddings.word_embeddings").get((cfg.vocab_size, h), "weight")?;
        let position_embeddings = vb.pp("embeddings.position_embeddings").get((cfg.max_position_embeddings, h), "weight")?;
        let token_type_embeddings = vb.pp("embeddings.token_type_embeddings").get((cfg.type_vocab_size, h), "weight")?;
        let emb_ln = LayerNorm::load(vb.pp("embeddings.LayerNorm"), h, cfg.layer_norm_eps)?;

        let mut layers = Vec::new();
        for i in 0..cfg.num_hidden_layers {
            layers.push(EncoderLayer::load(vb.pp(format!("encoder.layer.{i}")), &cfg)?);
        }

        Ok(BertModel { word_embeddings, position_embeddings, token_type_embeddings, emb_ln, layers, device })
    }

    /// ids/mask/type_ids: single sequence (batch = 1). Returns hidden states (1, s, h).
    pub fn forward(&self, ids: &[u32], mask: &[u32], type_ids: &[u32]) -> Result<Tensor> {
        let s = ids.len();
        let h = self.word_embeddings.dim(1)?;
        let ids_t = Tensor::from_slice(ids, (s,), &self.device)?;
        let type_t = Tensor::from_slice(type_ids, (s,), &self.device)?;
        let pos = Tensor::arange(0i64, s as i64, &self.device)?;

        let w = self.word_embeddings.embedding(&ids_t)?.reshape((1, s, h))?;
        let p = self.position_embeddings.embedding(&pos)?.reshape((1, s, h))?;
        let tt = self.token_type_embeddings.embedding(&type_t)?.reshape((1, s, h))?;
        let emb = ((&w + &p)? + &tt)?;
        let mut hidden = self.emb_ln.forward(&emb)?;

        // extended attention mask (1, 1, 1, s): 0 for real tokens, -1e9 for padding
        let mask_f = Tensor::from_slice(mask, (1, s), &self.device)?.to_dtype(DType::F32)?;
        let ext_mask = ((1.0 - mask_f)? * -1e9)?.reshape((1, 1, 1, s))?;

        for layer in &self.layers {
            hidden = layer.forward(&hidden, &ext_mask)?;
        }
        Ok(hidden)
    }
}