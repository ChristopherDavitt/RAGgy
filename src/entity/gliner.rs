use anyhow::{Result, Context};
use ort::value::Tensor;
use regex::Regex;
use std::path::Path;

/// A detected entity span from GLiNER.
#[derive(Debug, Clone)]
pub struct GlinerEntity {
    pub text: String,
    pub label: String,
    pub score: f32,
    pub start: usize, // byte offset in original text
    pub end: usize,   // byte offset in original text
}

/// Word-level token with byte offsets into the original text.
struct WordToken {
    text: String,
    start: usize,
    end: usize,
}

/// GLiNER NER model using ONNX Runtime.
pub struct GlinerModel {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    max_width: usize,
}

impl GlinerModel {
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
                "GLiNER model not found at {}. Download gliner_small-v2.1:\n\
                 \n\
                 mkdir -p {0}\n\
                 # Download model.onnx from https://huggingface.co/onnx-community/gliner_small-v2.1\n\
                 # Download tokenizer.json from https://huggingface.co/urchade/gliner_small-v2.1",
                model_dir.display(),
            );
        }

        if verbose {
            eprintln!("  Loading GLiNER model...");
        }
        let start = std::time::Instant::now();

        let builder = ort::session::Session::builder()
            .context("Failed to create ONNX session builder")?;
        let mut builder = builder.with_intra_threads(4)
            .map_err(|e| anyhow::anyhow!("Failed to set thread count: {}", e))?;
        let session = builder.commit_from_file(&model_path)
            .context("Failed to load GLiNER ONNX model")?;

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        if verbose {
            eprintln!("  GLiNER loaded in {:.1}s", start.elapsed().as_secs_f64());
        }

        Ok(GlinerModel {
            session,
            tokenizer,
            max_width: 12,
        })
    }

    /// Extract entities from text given a set of labels.
    pub fn extract(
        &mut self,
        text: &str,
        labels: &[String],
        threshold: f32,
    ) -> Result<Vec<GlinerEntity>> {
        if text.is_empty() || labels.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Word-level tokenization
        let words = word_tokenize(text);
        if words.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Build prompt: [<<ENT>>, label1, <<ENT>>, label2, ..., <<SEP>>, word1, word2, ...]
        let ent_token_id = self.get_token_id("<<ENT>>");
        let sep_token_id = self.get_token_id("<<SEP>>");
        let bos_id: i64 = 1;
        let eos_id: i64 = 2;

        // Encode each label and word individually
        let mut prompt_ids: Vec<i64> = Vec::new();
        let mut prompt_mask: Vec<i64> = Vec::new();
        let mut words_mask_vals: Vec<i64> = Vec::new();

        // BOS
        prompt_ids.push(bos_id);
        prompt_mask.push(1);
        words_mask_vals.push(0);

        // Entity labels prefix
        for label in labels {
            // <<ENT>> token
            prompt_ids.push(ent_token_id);
            prompt_mask.push(1);
            words_mask_vals.push(0);

            // Label sub-tokens
            let enc = self.tokenizer.encode(label.as_str(), false)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;
            for &id in enc.get_ids() {
                prompt_ids.push(id as i64);
                prompt_mask.push(1);
                words_mask_vals.push(0);
            }
        }

        // <<SEP>> token
        prompt_ids.push(sep_token_id);
        prompt_mask.push(1);
        words_mask_vals.push(0);

        let _text_offset = prompt_ids.len(); // where text tokens start

        // Text words
        let mut word_idx: i64 = 1; // 1-indexed
        for word in &words {
            let enc = self.tokenizer.encode(word.text.as_str(), false)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;
            let ids = enc.get_ids();
            for (j, &id) in ids.iter().enumerate() {
                prompt_ids.push(id as i64);
                prompt_mask.push(1);
                // Only first sub-token gets the word index
                words_mask_vals.push(if j == 0 { word_idx } else { 0 });
            }
            word_idx += 1;
        }

        // EOS
        prompt_ids.push(eos_id);
        prompt_mask.push(1);
        words_mask_vals.push(0);

        let seq_len = prompt_ids.len();
        let num_words = words.len();
        let num_labels = labels.len();

        // 3. Build span indices
        let num_spans = num_words * self.max_width;
        let mut span_idx_data = vec![0i64; num_spans * 2];
        let mut span_mask_data = vec![false; num_spans];

        for start in 0..num_words {
            for width in 0..self.max_width {
                let end = start + width;
                let span_i = start * self.max_width + width;
                if end < num_words {
                    span_idx_data[span_i * 2] = start as i64;
                    span_idx_data[span_i * 2 + 1] = end as i64;
                    span_mask_data[span_i] = true;
                }
            }
        }

        // 4. Build tensors (batch_size = 1)
        let input_ids = Tensor::from_array(
            (vec![1i64, seq_len as i64], prompt_ids)
        ).context("Failed to create input_ids")?;

        let attention_mask = Tensor::from_array(
            (vec![1i64, seq_len as i64], prompt_mask)
        ).context("Failed to create attention_mask")?;

        let words_mask = Tensor::from_array(
            (vec![1i64, seq_len as i64], words_mask_vals)
        ).context("Failed to create words_mask")?;

        let text_lengths = Tensor::from_array(
            (vec![1i64, 1i64], vec![num_words as i64])
        ).context("Failed to create text_lengths")?;

        let span_idx = Tensor::from_array(
            (vec![1i64, num_spans as i64, 2i64], span_idx_data)
        ).context("Failed to create span_idx")?;

        let span_mask_u8: Vec<u8> = span_mask_data.iter().map(|&b| b as u8).collect();
        let span_mask = Tensor::from_array(
            (vec![1i64, num_spans as i64], span_mask_u8)
        ).context("Failed to create span_mask")?;

        // 5. Run inference
        let outputs = self.session.run(ort::inputs![
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
            "words_mask" => words_mask,
            "text_lengths" => text_lengths,
            "span_idx" => span_idx,
            "span_mask" => span_mask,
        ])?;

        // 6. Decode output: logits shape [1, num_words, max_width, num_labels]
        let (_output_shape, output_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("Failed to extract output tensor")?;

        let mut entities = Vec::new();

        // Iterate over all (start, width, label) combinations
        for start in 0..num_words {
            for width in 0..self.max_width {
                let end = start + width;
                if end >= num_words {
                    break;
                }
                for label_idx in 0..num_labels {
                    let offset = start * self.max_width * num_labels
                        + width * num_labels
                        + label_idx;
                    let logit = output_data[offset];
                    let score = sigmoid(logit);

                    if score >= threshold {
                        // Map word indices back to byte offsets
                        let byte_start = words[start].start;
                        let byte_end = words[end].end;
                        let entity_text = &text[byte_start..byte_end];

                        entities.push(GlinerEntity {
                            text: entity_text.to_string(),
                            label: labels[label_idx].clone(),
                            score,
                            start: byte_start,
                            end: byte_end,
                        });
                    }
                }
            }
        }

        // 7. Sort by score descending, then greedy non-overlapping filter
        entities.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let entities = greedy_dedup(entities);

        Ok(entities)
    }

    /// Look up a token ID from the tokenizer vocabulary.
    fn get_token_id(&self, token: &str) -> i64 {
        self.tokenizer.token_to_id(token).unwrap_or(0) as i64
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Split text into word-level tokens with byte offsets.
fn word_tokenize(text: &str) -> Vec<WordToken> {
    let re = Regex::new(r"\w+(?:[-_]\w+)*|\S").unwrap();
    re.find_iter(text)
        .map(|m| WordToken {
            text: m.as_str().to_string(),
            start: m.start(),
            end: m.end(),
        })
        .collect()
}

/// Greedy deduplication: remove overlapping spans, keeping highest-scored.
fn greedy_dedup(sorted_entities: Vec<GlinerEntity>) -> Vec<GlinerEntity> {
    let mut result = Vec::new();
    for entity in sorted_entities {
        let overlaps = result.iter().any(|e: &GlinerEntity| {
            entity.start < e.end && entity.end > e.start
        });
        if !overlaps {
            result.push(entity);
        }
    }
    // Sort by position for stable output
    result.sort_by_key(|e| e.start);
    result
}
