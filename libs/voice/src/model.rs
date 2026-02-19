use crate::quant::*;
use crate::tensor::{RawTensor, Tensor};
use std::collections::HashMap;
use std::io::{self, Read, Seek};

const GGML_FILE_MAGIC: u32 = 0x67676d6c;

fn should_preserve_raw_weight_type() -> bool {
    let parse_truthy = |v: &str| {
        let v = v.trim().to_ascii_lowercase();
        !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
    };

    if let Ok(backend) = std::env::var("MAKEPAD_VOICE_BACKEND") {
        if backend.trim().eq_ignore_ascii_case("metal") {
            return true;
        }
        if backend.trim().eq_ignore_ascii_case("cpu") {
            return false;
        }
    }

    if let Ok(v) = std::env::var("MAKEPAD_VOICE_PRESERVE_WTYPE") {
        return parse_truthy(&v);
    }

    if let Ok(v) = std::env::var("MAKEPAD_VOICE_METAL") {
        return parse_truthy(&v);
    }

    cfg!(target_os = "macos")
}

#[derive(Debug, Clone)]
pub struct WhisperHparams {
    pub n_vocab: i32,
    pub n_audio_ctx: i32,
    pub n_audio_state: i32,
    pub n_audio_head: i32,
    pub n_audio_layer: i32,
    pub n_text_ctx: i32,
    pub n_text_state: i32,
    pub n_text_head: i32,
    pub n_text_layer: i32,
    pub n_mels: i32,
    pub ftype: i32,
}

#[derive(Debug, Clone)]
pub struct MelFilters {
    pub n_mel: i32,
    pub n_fft: i32,
    pub data: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct Vocab {
    pub n_vocab: i32,
    pub token_to_id: HashMap<String, i32>,
    pub id_to_token: HashMap<i32, String>,
    // special tokens
    pub token_eot: i32,
    pub token_sot: i32,
    pub token_translate: i32,
    pub token_transcribe: i32,
    pub token_solm: i32,
    pub token_prev: i32,
    pub token_nosp: i32,
    pub token_not: i32,
    pub token_beg: i32,
}

impl Vocab {
    pub fn is_multilingual(&self) -> bool {
        self.n_vocab >= 51865
    }

    pub fn num_languages(&self) -> i32 {
        self.n_vocab - 51765 - if self.is_multilingual() { 1 } else { 0 }
    }

    pub fn token_to_str(&self, id: i32) -> &str {
        self.id_to_token.get(&id).map(|s| s.as_str()).unwrap_or("")
    }
}

pub struct EncoderLayer {
    pub attn_ln_0_w: Tensor, // layer norm scale (element-wise)
    pub attn_ln_0_b: Tensor,
    pub attn_ln_1_w: RawTensor, // attn.out.weight (matmul)
    pub attn_ln_1_b: Tensor,    // attn.out.bias
    pub attn_q_w: RawTensor,
    pub attn_q_b: Tensor,
    pub attn_k_w: RawTensor,
    pub attn_v_w: RawTensor,
    pub attn_v_b: Tensor,
    pub mlp_ln_w: Tensor, // layer norm scale (element-wise)
    pub mlp_ln_b: Tensor,
    pub mlp_0_w: RawTensor,
    pub mlp_0_b: Tensor,
    pub mlp_1_w: RawTensor,
    pub mlp_1_b: Tensor,
}

pub struct DecoderLayer {
    // self-attention
    pub attn_ln_0_w: Tensor, // layer norm (element-wise)
    pub attn_ln_0_b: Tensor,
    pub attn_ln_1_w: RawTensor, // attn.out.weight (matmul)
    pub attn_ln_1_b: Tensor,
    pub attn_q_w: RawTensor,
    pub attn_q_b: Tensor,
    pub attn_k_w: RawTensor,
    pub attn_v_w: RawTensor,
    pub attn_v_b: Tensor,
    // cross-attention
    pub cross_attn_ln_0_w: Tensor, // layer norm (element-wise)
    pub cross_attn_ln_0_b: Tensor,
    pub cross_attn_ln_1_w: RawTensor, // cross_attn.out.weight (matmul)
    pub cross_attn_ln_1_b: Tensor,
    pub cross_attn_q_w: RawTensor,
    pub cross_attn_q_b: Tensor,
    pub cross_attn_k_w: RawTensor,
    pub cross_attn_v_w: RawTensor,
    pub cross_attn_v_b: Tensor,
    // feed-forward
    pub mlp_ln_w: Tensor, // layer norm (element-wise)
    pub mlp_ln_b: Tensor,
    pub mlp_0_w: RawTensor,
    pub mlp_0_b: Tensor,
    pub mlp_1_w: RawTensor,
    pub mlp_1_b: Tensor,
}

pub struct WhisperModel {
    pub hparams: WhisperHparams,
    pub filters: MelFilters,
    pub vocab: Vocab,
    pub wtype: u32,

    // encoder
    pub e_pe: Tensor,       // positional embedding [n_audio_ctx, n_audio_state]
    pub e_conv_1_w: Tensor, // conv1.weight [n_audio_state, n_mels, 3]
    pub e_conv_1_b: Tensor, // conv1.bias [n_audio_state]
    pub e_conv_2_w: Tensor, // conv2.weight [n_audio_state, n_audio_state, 3]
    pub e_conv_2_b: Tensor, // conv2.bias [n_audio_state]
    pub e_ln_w: Tensor,     // ln_post.weight [n_audio_state]
    pub e_ln_b: Tensor,     // ln_post.bias [n_audio_state]
    pub encoder_layers: Vec<EncoderLayer>,

    // decoder
    pub d_pe: Tensor,   // positional embedding [n_text_ctx, n_text_state]
    pub d_te: Tensor,   // token embedding [n_vocab, n_text_state]
    pub d_ln_w: Tensor, // ln.weight [n_text_state]
    pub d_ln_b: Tensor, // ln.bias [n_text_state]
    pub decoder_layers: Vec<DecoderLayer>,
}

fn read_i32<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_f32_vec<R: Read>(r: &mut R, n: usize) -> io::Result<Vec<f32>> {
    let mut buf = vec![0u8; n * 4];
    r.read_exact(&mut buf)?;
    let data: Vec<f32> = buf
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(data)
}

fn read_string<R: Read>(r: &mut R, len: usize) -> io::Result<String> {
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn ggml_type_for_ftype(ftype: i32) -> u32 {
    // ftype encodes the quantization format for the weight tensors
    match ftype % 16384 {
        // The quantization version factor
        0 => GGML_TYPE_F32,
        1 => GGML_TYPE_F16,
        2 => GGML_TYPE_Q4_0,
        3 => GGML_TYPE_Q4_1,
        6 => GGML_TYPE_Q5_0,
        7 => GGML_TYPE_Q5_1,
        8 => GGML_TYPE_Q8_0,
        _ => GGML_TYPE_F16, // fallback
    }
}

impl WhisperModel {
    pub fn load<R: Read + Seek>(r: &mut R) -> io::Result<Self> {
        // 1. Magic
        let magic = read_u32(r)?;
        if magic != GGML_FILE_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad magic: 0x{:x}", magic),
            ));
        }

        // 2. Hyperparameters
        let hparams = WhisperHparams {
            n_vocab: read_i32(r)?,
            n_audio_ctx: read_i32(r)?,
            n_audio_state: read_i32(r)?,
            n_audio_head: read_i32(r)?,
            n_audio_layer: read_i32(r)?,
            n_text_ctx: read_i32(r)?,
            n_text_state: read_i32(r)?,
            n_text_head: read_i32(r)?,
            n_text_layer: read_i32(r)?,
            n_mels: read_i32(r)?,
            ftype: read_i32(r)?,
        };

        let wtype = ggml_type_for_ftype(hparams.ftype);

        // 3. Mel filters
        let n_mel = read_i32(r)?;
        let n_fft = read_i32(r)?;
        let mel_data = read_f32_vec(r, (n_mel * n_fft) as usize)?;
        let filters = MelFilters {
            n_mel,
            n_fft,
            data: mel_data,
        };

        // 4. Vocabulary
        let n_vocab_file = read_i32(r)?;
        let mut token_to_id = HashMap::new();
        let mut id_to_token = HashMap::new();

        for i in 0..n_vocab_file {
            let len = read_u32(r)? as usize;
            let word = if len > 0 {
                read_string(r, len)?
            } else {
                String::new()
            };
            token_to_id.insert(word.clone(), i);
            id_to_token.insert(i, word);
        }

        let mut vocab = Vocab {
            n_vocab: hparams.n_vocab,
            token_to_id,
            id_to_token,
            token_eot: 50256,
            token_sot: 50257,
            token_translate: 50357,
            token_transcribe: 50358,
            token_solm: 50359,
            token_prev: 50360,
            token_nosp: 50361,
            token_not: 50362,
            token_beg: 50363,
        };

        // Adjust for multilingual models
        if vocab.is_multilingual() {
            vocab.token_eot += 1;
            vocab.token_sot += 1;
            let dt = vocab.num_languages() - 98;
            vocab.token_translate += dt;
            vocab.token_transcribe += dt;
            vocab.token_solm += dt;
            vocab.token_prev += dt;
            vocab.token_nosp += dt;
            vocab.token_not += dt;
            vocab.token_beg += dt;
        }

        // Add extra tokens if needed
        if n_vocab_file < hparams.n_vocab {
            for i in n_vocab_file..hparams.n_vocab {
                let word = if i > vocab.token_beg {
                    format!("[_TT_{}]", i - vocab.token_beg)
                } else if i == vocab.token_eot {
                    "[_EOT_]".into()
                } else if i == vocab.token_sot {
                    "[_SOT_]".into()
                } else if i == vocab.token_beg {
                    "[_BEG_]".into()
                } else {
                    format!("[_extra_token_{}]", i)
                };
                vocab.token_to_id.insert(word.clone(), i);
                vocab.id_to_token.insert(i, word);
            }
        }

        // 5. Load tensors
        let mut tensors: HashMap<String, RawTensor> = HashMap::new();

        loop {
            let n_dims = match read_i32(r) {
                Ok(v) => v,
                Err(_) => break,
            };
            let name_len = match read_i32(r) {
                Ok(v) => v,
                Err(_) => break,
            };
            let ttype = match read_i32(r) {
                Ok(v) => v as u32,
                Err(_) => break,
            };

            let mut ne = vec![0i32; n_dims as usize];
            for i in 0..n_dims as usize {
                ne[i] = read_i32(r)?;
            }

            let name = read_string(r, name_len as usize)?;

            let nelements: usize = ne.iter().map(|&x| x as usize).product();
            let be = block_elements(ttype);
            let bs = block_size(ttype);
            let nbytes = (nelements / be) * bs;

            let mut data = vec![0u8; nbytes];
            r.read_exact(&mut data)?;

            // Keep GGML shape order (dim 0 = innermost/contiguous dimension).
            // For 2D: shape[0] = columns (contiguous), shape[1] = rows
            // For matmul weights [in, out]: row i of out_features has in_features contiguous elements
            let shape: Vec<usize> = ne.iter().map(|&x| x as usize).collect();

            tensors.insert(
                name,
                RawTensor {
                    data,
                    shape,
                    ggml_type: ttype,
                },
            );
        }

        // 6. Build model struct from loaded tensors
        fn take_f32(tensors: &mut HashMap<String, RawTensor>, name: &str) -> Tensor {
            tensors
                .remove(name)
                .unwrap_or_else(|| panic!("missing tensor: {}", name))
                .to_f32()
        }
        let preserve_wtype = should_preserve_raw_weight_type();
        fn take_raw(
            tensors: &mut HashMap<String, RawTensor>,
            name: &str,
            preserve_wtype: bool,
        ) -> RawTensor {
            let t = tensors
                .remove(name)
                .unwrap_or_else(|| panic!("missing tensor: {}", name));
            if preserve_wtype {
                t
            } else {
                t.to_q8_0()
            }
        }

        // Encoder global tensors
        let e_pe = take_f32(&mut tensors, "encoder.positional_embedding");
        let e_conv_1_w = take_f32(&mut tensors, "encoder.conv1.weight");
        let e_conv_1_b = take_f32(&mut tensors, "encoder.conv1.bias");
        let e_conv_2_w = take_f32(&mut tensors, "encoder.conv2.weight");
        let e_conv_2_b = take_f32(&mut tensors, "encoder.conv2.bias");
        let e_ln_w = take_f32(&mut tensors, "encoder.ln_post.weight");
        let e_ln_b = take_f32(&mut tensors, "encoder.ln_post.bias");

        // Encoder layers
        let mut encoder_layers = Vec::new();
        for i in 0..hparams.n_audio_layer {
            let p = format!("encoder.blocks.{}", i);
            encoder_layers.push(EncoderLayer {
                attn_ln_0_w: take_f32(&mut tensors, &format!("{}.attn_ln.weight", p)),
                attn_ln_0_b: take_f32(&mut tensors, &format!("{}.attn_ln.bias", p)),
                attn_ln_1_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.out.weight", p),
                    preserve_wtype,
                ),
                attn_ln_1_b: take_f32(&mut tensors, &format!("{}.attn.out.bias", p)),
                attn_q_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.query.weight", p),
                    preserve_wtype,
                ),
                attn_q_b: take_f32(&mut tensors, &format!("{}.attn.query.bias", p)),
                attn_k_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.key.weight", p),
                    preserve_wtype,
                ),
                attn_v_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.value.weight", p),
                    preserve_wtype,
                ),
                attn_v_b: take_f32(&mut tensors, &format!("{}.attn.value.bias", p)),
                mlp_ln_w: take_f32(&mut tensors, &format!("{}.mlp_ln.weight", p)),
                mlp_ln_b: take_f32(&mut tensors, &format!("{}.mlp_ln.bias", p)),
                mlp_0_w: take_raw(&mut tensors, &format!("{}.mlp.0.weight", p), preserve_wtype),
                mlp_0_b: take_f32(&mut tensors, &format!("{}.mlp.0.bias", p)),
                mlp_1_w: take_raw(&mut tensors, &format!("{}.mlp.2.weight", p), preserve_wtype),
                mlp_1_b: take_f32(&mut tensors, &format!("{}.mlp.2.bias", p)),
            });
        }

        // Decoder global tensors
        let d_pe = take_f32(&mut tensors, "decoder.positional_embedding");
        let d_te = take_f32(&mut tensors, "decoder.token_embedding.weight");
        let d_ln_w = take_f32(&mut tensors, "decoder.ln.weight");
        let d_ln_b = take_f32(&mut tensors, "decoder.ln.bias");

        // Decoder layers
        let mut decoder_layers = Vec::new();
        for i in 0..hparams.n_text_layer {
            let p = format!("decoder.blocks.{}", i);
            decoder_layers.push(DecoderLayer {
                attn_ln_0_w: take_f32(&mut tensors, &format!("{}.attn_ln.weight", p)),
                attn_ln_0_b: take_f32(&mut tensors, &format!("{}.attn_ln.bias", p)),
                attn_ln_1_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.out.weight", p),
                    preserve_wtype,
                ),
                attn_ln_1_b: take_f32(&mut tensors, &format!("{}.attn.out.bias", p)),
                attn_q_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.query.weight", p),
                    preserve_wtype,
                ),
                attn_q_b: take_f32(&mut tensors, &format!("{}.attn.query.bias", p)),
                attn_k_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.key.weight", p),
                    preserve_wtype,
                ),
                attn_v_w: take_raw(
                    &mut tensors,
                    &format!("{}.attn.value.weight", p),
                    preserve_wtype,
                ),
                attn_v_b: take_f32(&mut tensors, &format!("{}.attn.value.bias", p)),
                cross_attn_ln_0_w: take_f32(&mut tensors, &format!("{}.cross_attn_ln.weight", p)),
                cross_attn_ln_0_b: take_f32(&mut tensors, &format!("{}.cross_attn_ln.bias", p)),
                cross_attn_ln_1_w: take_raw(
                    &mut tensors,
                    &format!("{}.cross_attn.out.weight", p),
                    preserve_wtype,
                ),
                cross_attn_ln_1_b: take_f32(&mut tensors, &format!("{}.cross_attn.out.bias", p)),
                cross_attn_q_w: take_raw(
                    &mut tensors,
                    &format!("{}.cross_attn.query.weight", p),
                    preserve_wtype,
                ),
                cross_attn_q_b: take_f32(&mut tensors, &format!("{}.cross_attn.query.bias", p)),
                cross_attn_k_w: take_raw(
                    &mut tensors,
                    &format!("{}.cross_attn.key.weight", p),
                    preserve_wtype,
                ),
                cross_attn_v_w: take_raw(
                    &mut tensors,
                    &format!("{}.cross_attn.value.weight", p),
                    preserve_wtype,
                ),
                cross_attn_v_b: take_f32(&mut tensors, &format!("{}.cross_attn.value.bias", p)),
                mlp_ln_w: take_f32(&mut tensors, &format!("{}.mlp_ln.weight", p)),
                mlp_ln_b: take_f32(&mut tensors, &format!("{}.mlp_ln.bias", p)),
                mlp_0_w: take_raw(&mut tensors, &format!("{}.mlp.0.weight", p), preserve_wtype),
                mlp_0_b: take_f32(&mut tensors, &format!("{}.mlp.0.bias", p)),
                mlp_1_w: take_raw(&mut tensors, &format!("{}.mlp.2.weight", p), preserve_wtype),
                mlp_1_b: take_f32(&mut tensors, &format!("{}.mlp.2.bias", p)),
            });
        }

        Ok(WhisperModel {
            hparams,
            filters,
            vocab,
            wtype,
            e_pe,
            e_conv_1_w,
            e_conv_1_b,
            e_conv_2_w,
            e_conv_2_b,
            e_ln_w,
            e_ln_b,
            encoder_layers,
            d_pe,
            d_te,
            d_ln_w,
            d_ln_b,
            decoder_layers,
        })
    }

    pub fn load_file(path: &str) -> io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        Self::load(&mut f)
    }
}
