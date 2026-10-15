// The LLM host path with no GPU and no weights: what the CUDA decode runs on the CPU around the
// device launch (libs/ai/llm: host_bench over the session's own load and graph code). setup writes a
// small GGUF -- the Qwen3.8-27B architecture (qwen35 hybrid: delta-net recurrent layers, an attention
// layer every 4th, 64 blocks, the real 248,320-token vocab) with the widths scaled down, tensor
// infos only (the data region is an unwritten, sparse part of the file, never read) -- and maps it
// as a session does. Per iteration: build the decode graph for a key bucket (what a session does
// once per 256 tokens), then 256 decode tokens, each: layout validation, input writes (token ids,
// positions, attention mask, state rows), readback shaping of a fixed 1 MB logits vector,
// single-token collapse and greedy sampling. RW_LLM_GGUF=<path> runs a real GGUF instead (node 165:
// /home/arch/.makepad/weights/llm/Qwen3.8-27B-Q4_K_M.gguf).
// Not covered: the CUDA graph planner (plan_raw_graph), which compiles only with the CUDA kernels.
use crate::fold;
use makepad_ai_llm::host_bench::HostDecodeBench;
use std::io::Write;
use std::path::PathBuf;

const E: u64 = 256; // embedding
const F: u64 = 768; // feed forward
const HEADS: u64 = 8;
const KV_HEADS: u64 = 2;
const HEAD_DIM: u64 = 64;
const SSM_STATE: u64 = 32;
const SSM_GROUPS: u64 = 4;
const SSM_RANK: u64 = 8;
const SSM_INNER: u64 = 256;
const BLOCKS: usize = 64;
const VOCAB: u64 = 248_320;

const T_F32: i32 = 0;
const T_Q4K: i32 = 12;
const T_Q5K: i32 = 13;
const T_Q6K: i32 = 14;

fn row_bytes(ty: i32, ne0: u64) -> u64 {
    match ty {
        T_F32 => ne0 * 4,
        T_Q4K => ne0 / 256 * 144,
        T_Q5K => ne0 / 256 * 176,
        T_Q6K => ne0 / 256 * 210,
        _ => unreachable!(),
    }
}

struct Gguf {
    kv: Vec<u8>,
    n_kv: u64,
    tensors: Vec<(String, Vec<u64>, i32)>,
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u64).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

impl Gguf {
    fn key(&mut self, k: &str, ty: i32) {
        put_str(&mut self.kv, k);
        self.kv.extend_from_slice(&ty.to_le_bytes());
        self.n_kv += 1;
    }
    fn string(&mut self, k: &str, v: &str) {
        self.key(k, 8);
        put_str(&mut self.kv, v);
    }
    fn u32(&mut self, k: &str, v: u32) {
        self.key(k, 4);
        self.kv.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, k: &str, v: f32) {
        self.key(k, 6);
        self.kv.extend_from_slice(&v.to_le_bytes());
    }
    fn u32_array(&mut self, k: &str, v: &[u32]) {
        self.key(k, 9);
        self.kv.extend_from_slice(&4i32.to_le_bytes());
        self.kv.extend_from_slice(&(v.len() as u64).to_le_bytes());
        for x in v {
            self.kv.extend_from_slice(&x.to_le_bytes());
        }
    }
    fn tensor(&mut self, name: String, dims: &[u64], ty: i32) {
        self.tensors.push((name, dims.to_vec(), ty));
    }

    fn write(&self, path: &std::path::Path) {
        const ALIGN: u64 = 32;
        let mut head = Vec::new();
        head.extend_from_slice(b"GGUF");
        head.extend_from_slice(&3u32.to_le_bytes());
        head.extend_from_slice(&(self.tensors.len() as u64).to_le_bytes());
        head.extend_from_slice(&self.n_kv.to_le_bytes());
        head.extend_from_slice(&self.kv);
        let mut offset = 0u64;
        for (name, dims, ty) in &self.tensors {
            put_str(&mut head, name);
            head.extend_from_slice(&(dims.len() as u32).to_le_bytes());
            for d in dims {
                head.extend_from_slice(&d.to_le_bytes());
            }
            head.extend_from_slice(&ty.to_le_bytes());
            head.extend_from_slice(&offset.to_le_bytes());
            let size = row_bytes(*ty, dims[0]) * dims[1..].iter().product::<u64>();
            offset = (offset + size).div_ceil(ALIGN) * ALIGN;
        }
        let data_start = (head.len() as u64).div_ceil(ALIGN) * ALIGN;
        head.resize(data_start as usize, 0);
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(&head).unwrap();
        f.set_len(data_start + offset).unwrap();
    }
}

fn synthetic() -> Gguf {
    let mut g = Gguf { kv: Vec::new(), n_kv: 0, tensors: Vec::new() };
    g.string("general.architecture", "qwen35");
    g.string("general.name", "rwbench-qwen35-scaled");
    g.u32("general.file_type", 15);
    g.u32("general.quantization_version", 2);
    g.u32("qwen35.block_count", BLOCKS as u32);
    g.u32("qwen35.context_length", 262_144);
    g.u32("qwen35.embedding_length", E as u32);
    g.u32("qwen35.feed_forward_length", F as u32);
    g.u32("qwen35.attention.head_count", HEADS as u32);
    g.u32("qwen35.attention.head_count_kv", KV_HEADS as u32);
    g.u32_array("qwen35.rope.dimension_sections", &[6, 5, 5, 0]);
    g.f32("qwen35.rope.freq_base", 10_000_000.0);
    g.f32("qwen35.attention.layer_norm_rms_epsilon", 1e-6);
    g.u32("qwen35.attention.key_length", HEAD_DIM as u32);
    g.u32("qwen35.attention.value_length", HEAD_DIM as u32);
    g.u32("qwen35.nextn_predict_layers", 0);
    g.u32("qwen35.ssm.conv_kernel", 4);
    g.u32("qwen35.ssm.state_size", SSM_STATE as u32);
    g.u32("qwen35.ssm.group_count", SSM_GROUPS as u32);
    g.u32("qwen35.ssm.time_step_rank", SSM_RANK as u32);
    g.u32("qwen35.ssm.inner_size", SSM_INNER as u32);
    g.u32("qwen35.full_attention_interval", 4);
    g.u32("qwen35.rope.dimension_count", 32);
    g.tensor("output.weight".into(), &[E, VOCAB], T_Q6K);
    g.tensor("output_norm.weight".into(), &[E], T_F32);
    g.tensor("token_embd.weight".into(), &[E, VOCAB], T_Q4K);
    let conv_dim = SSM_INNER + 2 * SSM_GROUPS * SSM_STATE;
    for i in 0..BLOCKS {
        let n = |s: &str| format!("blk.{i}.{s}");
        if (i + 1) % 4 == 0 {
            g.tensor(n("attn_k.weight"), &[E, KV_HEADS * HEAD_DIM], T_Q4K);
            g.tensor(n("attn_k_norm.weight"), &[HEAD_DIM], T_F32);
            g.tensor(n("attn_output.weight"), &[HEADS * HEAD_DIM, E], T_Q4K);
            g.tensor(n("attn_q.weight"), &[E, HEADS * HEAD_DIM * 2], T_Q4K);
            g.tensor(n("attn_q_norm.weight"), &[HEAD_DIM], T_F32);
            g.tensor(n("attn_v.weight"), &[E, KV_HEADS * HEAD_DIM], T_Q6K);
        } else {
            g.tensor(n("attn_gate.weight"), &[E, SSM_INNER], T_Q4K);
            g.tensor(n("attn_qkv.weight"), &[E, conv_dim], T_Q6K);
            g.tensor(n("ssm_a"), &[SSM_RANK], T_F32);
            g.tensor(n("ssm_alpha.weight"), &[E, SSM_RANK], T_F32);
            g.tensor(n("ssm_beta.weight"), &[E, SSM_RANK], T_F32);
            g.tensor(n("ssm_conv1d.weight"), &[4, conv_dim], T_F32);
            g.tensor(n("ssm_dt.bias"), &[SSM_RANK], T_F32);
            g.tensor(n("ssm_norm.weight"), &[SSM_STATE], T_F32);
            g.tensor(n("ssm_out.weight"), &[SSM_INNER, E], T_Q5K);
        }
        g.tensor(n("attn_norm.weight"), &[E], T_F32);
        g.tensor(n("ffn_down.weight"), &[F, E], T_Q6K);
        g.tensor(n("ffn_gate.weight"), &[E, F], T_Q4K);
        g.tensor(n("ffn_up.weight"), &[E, F], T_Q4K);
        g.tensor(n("post_attention_norm.weight"), &[E], T_F32);
    }
    g
}

struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn setup() -> Box<dyn FnMut() -> u64> {
    let (path, guard) = match std::env::var("RW_LLM_GGUF") {
        Ok(p) => (PathBuf::from(p), None),
        Err(_) => {
            let p = std::env::temp_dir().join(format!("rwbench-llm-{}.gguf", std::process::id()));
            synthetic().write(&p);
            (p.clone(), Some(TempFile(p)))
        }
    };
    let mut bench = HostDecodeBench::new(&path, 4096).unwrap_or_else(|e| panic!("llm_host setup: {:?}", e));
    Box::new(move || {
        let _keep = &guard;
        let t0 = std::time::Instant::now();
        let mut h = bench.build_graph(1025).unwrap();
        let t1 = std::time::Instant::now();
        let mut token = 1i32;
        for pos in 1024..1280usize {
            let (next, d) = bench.step(token, pos).unwrap();
            h = fold(h, d);
            token = next;
        }
        if std::env::var("RW_DEBUG").is_ok() {
            eprintln!("llm_host: graph build {:.2} ms, 256 tokens {:.2} ms", (t1 - t0).as_secs_f64() * 1e3, t1.elapsed().as_secs_f64() * 1e3);
        }
        h
    })
}
