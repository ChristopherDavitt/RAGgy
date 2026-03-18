use anyhow::{Result, Context};
use ort::value::Tensor;
use std::path::Path;

pub struct Embedder {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
}

impl Embedder {
    pub fn load(model_dir: &Path) -> Result<Self> {
        Self::load_inner(model_dir, false)
    }

    pub fn load_verbose(model_dir: &Path) -> Result<Self> {
        Self::load_inner(model_dir, true)
    }

    fn load_inner(model_dir: &Path, verbose: bool) -> Result<Self> {
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        if !model_path.exists() || !tokenizer_path.exists() {
            anyhow::bail!(
                "Embedding model not found. Download all-MiniLM-L6-v2 to {}:\n\
                 \n\
                 mkdir -p {}\n\
                 curl -L -o {}/model.onnx \\\n\
                   https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx\n\
                 curl -L -o {}/tokenizer.json \\\n\
                   https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
                model_dir.display(),
                model_dir.display(),
                model_dir.display(),
                model_dir.display(),
            );
        }

        if verbose { eprintln!("  Loading ONNX model..."); }
        let start = std::time::Instant::now();
        let builder = ort::session::Session::builder()
            .context("Failed to create ONNX session builder")?;
        let mut builder = builder.with_intra_threads(4)
            .map_err(|e| anyhow::anyhow!("Failed to set thread count: {}", e))?;
        let session = builder.commit_from_file(&model_path)
            .context("Failed to load ONNX model")?;
        if verbose { eprintln!("  Model loaded in {:.1}s", start.elapsed().as_secs_f64()); }

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        Ok(Embedder { session, tokenizer })
    }

    /// Embed a single text string into a 384-dimensional vector.
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let batch = self.embed_batch(&[text])?;
        Ok(batch.into_iter().next().unwrap())
    }

    /// Embed a batch of texts. Returns one Vec<f32> per input.
    pub fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let max_len: usize = 256; // all-MiniLM-L6-v2 max sequence length

        // Tokenize all texts
        let encodings: Vec<tokenizers::Encoding> = texts
            .iter()
            .map(|t| {
                self.tokenizer
                    .encode(*t, true)
                    .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))
            })
            .collect::<Result<Vec<_>>>()?;

        // Find the max length in this batch (capped at max_len)
        let batch_max_len = encodings
            .iter()
            .map(|e| e.get_ids().len().min(max_len))
            .max()
            .unwrap_or(0);

        let batch_size = texts.len();

        // Build flat padded input tensors
        let mut input_ids_data = vec![0i64; batch_size * batch_max_len];
        let mut attention_mask_data = vec![0i64; batch_size * batch_max_len];
        let mut token_type_ids_data = vec![0i64; batch_size * batch_max_len];

        for (i, enc) in encodings.iter().enumerate() {
            let ids = enc.get_ids();
            let mask = enc.get_attention_mask();
            let types = enc.get_type_ids();
            let len = ids.len().min(batch_max_len);

            for j in 0..len {
                input_ids_data[i * batch_max_len + j] = ids[j] as i64;
                attention_mask_data[i * batch_max_len + j] = mask[j] as i64;
                token_type_ids_data[i * batch_max_len + j] = types[j] as i64;
            }
        }

        let shape = vec![batch_size as i64, batch_max_len as i64];

        let input_ids = Tensor::from_array((shape.clone(), input_ids_data.clone()))
            .context("Failed to create input_ids tensor")?;
        let attention_mask = Tensor::from_array((shape.clone(), attention_mask_data.clone()))
            .context("Failed to create attention_mask tensor")?;
        let token_type_ids = Tensor::from_array((shape.clone(), token_type_ids_data))
            .context("Failed to create token_type_ids tensor")?;

        // Run inference
        let outputs = self.session.run(ort::inputs![
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
            "token_type_ids" => token_type_ids,
        ])?;

        // Extract output tensor: shape [batch_size, seq_len, 384]
        let (output_shape, output_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("Failed to extract output tensor")?;

        let hidden_dim = output_shape[2] as usize; // 384
        let seq_len = output_shape[1] as usize;

        // Mean pooling with attention mask
        let mut results = Vec::with_capacity(batch_size);

        for i in 0..batch_size {
            let mut pooled = vec![0.0f32; hidden_dim];
            let mut mask_sum = 0.0f32;

            for j in 0..seq_len {
                let mask_val = attention_mask_data[i * batch_max_len + j] as f32;
                mask_sum += mask_val;
                let offset = i * seq_len * hidden_dim + j * hidden_dim;
                for k in 0..hidden_dim {
                    pooled[k] += output_data[offset + k] * mask_val;
                }
            }

            // Divide by mask sum
            if mask_sum > 0.0 {
                for v in &mut pooled {
                    *v /= mask_sum;
                }
            }

            // L2 normalize
            let norm: f32 = pooled.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut pooled {
                    *v /= norm;
                }
            }

            results.push(pooled);
        }

        Ok(results)
    }
}
