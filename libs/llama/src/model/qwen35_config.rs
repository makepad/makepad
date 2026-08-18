use crate::error::{LlamaError, Result};
use crate::gguf::{GgufFile, GgufValue};

use super::gguf_meta::{required_f32, required_u32, required_u32_array};

const NEXTN_PREDICT_LAYERS_KEY: &str = "qwen35.nextn_predict_layers";

#[derive(Clone, Debug)]
pub struct Qwen35Config {
    pub block_count: u32,
    pub context_length: u32,
    pub embedding_length: u32,
    pub feed_forward_length: u32,
    pub attention_head_count: u32,
    pub attention_head_count_kv: u32,
    pub attention_key_length: u32,
    pub attention_value_length: u32,
    pub rope_dimension_count: u32,
    pub rope_dimension_sections: Vec<u32>,
    pub rope_freq_base: f32,
    pub attention_layer_norm_rms_epsilon: f32,
    pub ssm_conv_kernel: u32,
    pub ssm_state_size: u32,
    pub ssm_group_count: u32,
    pub ssm_time_step_rank: u32,
    pub ssm_inner_size: u32,
    pub full_attention_interval: u32,
    /// Trailing multi-token-prediction (MTP/draft) blocks. They carry
    /// `blk.N.nextn.*` tensors, are not part of the main forward pass, and
    /// the main network is the first `block_count - nextn_predict_layers`
    /// blocks (for example Qwen3.8-27B: 65 blocks, 1 nextn layer).
    ///
    /// `from_gguf` requires `nextn_predict_layers < block_count`.
    pub nextn_predict_layers: u32,
}

impl Qwen35Config {
    /// Blocks executed by the main forward pass (excludes trailing MTP
    /// draft blocks). The struct is public, so revalidate the invariant at
    /// every use instead of relying only on [`Self::from_gguf`].
    pub fn main_block_count(&self) -> Result<u32> {
        validate_layer_counts(self.block_count, self.nextn_predict_layers)
    }

    pub fn from_gguf(gguf: &GgufFile) -> Result<Self> {
        let block_count = required_u32(gguf, "qwen35.block_count")?;
        let nextn_predict_layers = optional_nextn_predict_layers(gguf)?;
        validate_layer_counts(block_count, nextn_predict_layers)?;
        Ok(Self {
            block_count,
            context_length: required_u32(gguf, "qwen35.context_length")?,
            embedding_length: required_u32(gguf, "qwen35.embedding_length")?,
            feed_forward_length: required_u32(gguf, "qwen35.feed_forward_length")?,
            attention_head_count: required_u32(gguf, "qwen35.attention.head_count")?,
            attention_head_count_kv: required_u32(gguf, "qwen35.attention.head_count_kv")?,
            attention_key_length: required_u32(gguf, "qwen35.attention.key_length")?,
            attention_value_length: required_u32(gguf, "qwen35.attention.value_length")?,
            rope_dimension_count: required_u32(gguf, "qwen35.rope.dimension_count")?,
            rope_dimension_sections: required_u32_array(gguf, "qwen35.rope.dimension_sections")?,
            rope_freq_base: required_f32(gguf, "qwen35.rope.freq_base")?,
            attention_layer_norm_rms_epsilon: required_f32(
                gguf,
                "qwen35.attention.layer_norm_rms_epsilon",
            )?,
            ssm_conv_kernel: required_u32(gguf, "qwen35.ssm.conv_kernel")?,
            ssm_state_size: required_u32(gguf, "qwen35.ssm.state_size")?,
            ssm_group_count: required_u32(gguf, "qwen35.ssm.group_count")?,
            ssm_time_step_rank: required_u32(gguf, "qwen35.ssm.time_step_rank")?,
            ssm_inner_size: required_u32(gguf, "qwen35.ssm.inner_size")?,
            full_attention_interval: required_u32(gguf, "qwen35.full_attention_interval")?,
            nextn_predict_layers,
        })
    }
}

fn optional_nextn_predict_layers(gguf: &GgufFile) -> Result<u32> {
    let Some(value) = gguf.get_value(NEXTN_PREDICT_LAYERS_KEY) else {
        return Ok(0);
    };
    value_to_u32(value)
}

fn value_to_u32(value: &GgufValue) -> Result<u32> {
    match value {
        GgufValue::Uint8(v) => Ok(u32::from(*v)),
        GgufValue::Int8(v) => u32::try_from(*v).map_err(|_| out_of_range(value)),
        GgufValue::Uint16(v) => Ok(u32::from(*v)),
        GgufValue::Int16(v) => u32::try_from(*v).map_err(|_| out_of_range(value)),
        GgufValue::Uint32(v) => Ok(*v),
        GgufValue::Uint64(v) => u32::try_from(*v).map_err(|_| out_of_range(value)),
        GgufValue::Int32(v) => u32::try_from(*v).map_err(|_| out_of_range(value)),
        GgufValue::Int64(v) => u32::try_from(*v).map_err(|_| out_of_range(value)),
        other => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected integral scalar",
            NEXTN_PREDICT_LAYERS_KEY,
            other.value_type().name()
        ))),
    }
}

fn out_of_range(value: &GgufValue) -> LlamaError {
    LlamaError::format(format!(
        "gguf key '{}' value is outside the u32 range (type {})",
        NEXTN_PREDICT_LAYERS_KEY,
        value.value_type().name()
    ))
}

fn validate_layer_counts(block_count: u32, nextn_predict_layers: u32) -> Result<u32> {
    if nextn_predict_layers >= block_count {
        return Err(LlamaError::format(format!(
            "qwen35.nextn_predict_layers {} must be less than qwen35.block_count {}",
            nextn_predict_layers, block_count
        )));
    }
    Ok(block_count - nextn_predict_layers)
}

#[cfg(test)]
mod tests {
    use super::{value_to_u32, Qwen35Config, NEXTN_PREDICT_LAYERS_KEY};
    use crate::error::LlamaError;
    use crate::gguf::{GgufFile, GgufValue};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Clone)]
    enum TestValue {
        U32(u32),
        I32(i32),
        U64(u64),
        I64(i64),
        F32(f32),
        U32Array(Vec<u32>),
    }

    struct TempGguf(PathBuf);

    impl Drop for TempGguf {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn push_str(buf: &mut Vec<u8>, value: &str) {
        buf.extend_from_slice(&(value.len() as u64).to_le_bytes());
        buf.extend_from_slice(value.as_bytes());
    }

    fn encode_gguf(kvs: &[(&str, TestValue)]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GGUF");
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&0i64.to_le_bytes());
        buf.extend_from_slice(&(kvs.len() as i64).to_le_bytes());
        for (key, value) in kvs {
            push_str(&mut buf, key);
            match value {
                TestValue::U32(v) => {
                    buf.extend_from_slice(&4i32.to_le_bytes());
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                TestValue::I32(v) => {
                    buf.extend_from_slice(&5i32.to_le_bytes());
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                TestValue::F32(v) => {
                    buf.extend_from_slice(&6i32.to_le_bytes());
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                TestValue::U32Array(values) => {
                    buf.extend_from_slice(&9i32.to_le_bytes());
                    buf.extend_from_slice(&4i32.to_le_bytes());
                    buf.extend_from_slice(&(values.len() as u64).to_le_bytes());
                    for item in values {
                        buf.extend_from_slice(&item.to_le_bytes());
                    }
                }
                TestValue::U64(v) => {
                    buf.extend_from_slice(&10i32.to_le_bytes());
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                TestValue::I64(v) => {
                    buf.extend_from_slice(&11i32.to_le_bytes());
                    buf.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        buf
    }

    fn required_qwen35_kvs(block_count: u32) -> Vec<(&'static str, TestValue)> {
        vec![
            ("qwen35.block_count", TestValue::U32(block_count)),
            ("qwen35.context_length", TestValue::U32(4096)),
            ("qwen35.embedding_length", TestValue::U32(1024)),
            ("qwen35.feed_forward_length", TestValue::U32(2048)),
            ("qwen35.attention.head_count", TestValue::U32(16)),
            ("qwen35.attention.head_count_kv", TestValue::U32(2)),
            ("qwen35.attention.key_length", TestValue::U32(64)),
            ("qwen35.attention.value_length", TestValue::U32(64)),
            ("qwen35.rope.dimension_count", TestValue::U32(64)),
            (
                "qwen35.rope.dimension_sections",
                TestValue::U32Array(vec![16, 24, 24, 16]),
            ),
            ("qwen35.rope.freq_base", TestValue::F32(1_000_000.0)),
            (
                "qwen35.attention.layer_norm_rms_epsilon",
                TestValue::F32(1.0e-6),
            ),
            ("qwen35.ssm.conv_kernel", TestValue::U32(4)),
            ("qwen35.ssm.state_size", TestValue::U32(128)),
            ("qwen35.ssm.group_count", TestValue::U32(4)),
            ("qwen35.ssm.time_step_rank", TestValue::U32(16)),
            ("qwen35.ssm.inner_size", TestValue::U32(512)),
            ("qwen35.full_attention_interval", TestValue::U32(4)),
        ]
    }

    fn parse_qwen35(
        block_count: u32,
        nextn: Option<TestValue>,
    ) -> Result<Qwen35Config, LlamaError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut kvs = required_qwen35_kvs(block_count);
        if let Some(value) = nextn {
            kvs.push((NEXTN_PREDICT_LAYERS_KEY, value));
        }
        let bytes = encode_gguf(&kvs);
        let path = std::env::temp_dir().join(format!(
            "makepad-llama-qwen35-nextn-{}-{}.gguf",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).expect("write hermetic gguf");
        let _temp = TempGguf(path.clone());
        Qwen35Config::from_gguf(&GgufFile::open(&path)?)
    }

    fn format_err(err: LlamaError) -> String {
        match err {
            LlamaError::Format(msg) => msg,
            other => panic!("expected format error, got {other:?}"),
        }
    }

    #[test]
    fn nextn_absent_defaults_to_zero() {
        let cfg = parse_qwen35(65, None).expect("absent nextn should parse");
        assert_eq!(cfg.nextn_predict_layers, 0);
        assert_eq!(cfg.main_block_count().unwrap(), 65);
    }

    #[test]
    fn nextn_qwen38_65_1_parses() {
        let cfg = parse_qwen35(65, Some(TestValue::U32(1))).expect("valid Qwen3.8 65/1");
        assert_eq!(cfg.block_count, 65);
        assert_eq!(cfg.nextn_predict_layers, 1);
        assert_eq!(cfg.main_block_count().unwrap(), 64);
    }

    #[test]
    fn nextn_equal_to_block_count_is_rejected() {
        let msg = format_err(parse_qwen35(65, Some(TestValue::U32(65))).unwrap_err());
        assert!(
            msg.contains("qwen35.nextn_predict_layers 65")
                && msg.contains("must be less than")
                && msg.contains("qwen35.block_count 65"),
            "{msg}"
        );
    }

    #[test]
    fn nextn_greater_than_block_count_is_rejected() {
        let msg = format_err(parse_qwen35(65, Some(TestValue::U32(66))).unwrap_err());
        assert!(
            msg.contains("qwen35.nextn_predict_layers 66")
                && msg.contains("must be less than")
                && msg.contains("qwen35.block_count 65"),
            "{msg}"
        );
    }

    #[test]
    fn nextn_wrong_type_is_rejected() {
        let msg = format_err(parse_qwen35(65, Some(TestValue::F32(1.0))).unwrap_err());
        assert!(
            msg.contains(NEXTN_PREDICT_LAYERS_KEY)
                && msg.contains("f32")
                && msg.contains("expected integral scalar"),
            "{msg}"
        );
    }

    #[test]
    fn nextn_negative_or_out_of_range_is_rejected() {
        for value in [
            TestValue::I32(-1),
            TestValue::I64(-1),
            TestValue::U64(u64::from(u32::MAX) + 1),
        ] {
            let msg = format_err(parse_qwen35(65, Some(value)).unwrap_err());
            assert!(
                msg.contains(NEXTN_PREDICT_LAYERS_KEY)
                    && msg.contains("outside the u32 range"),
                "{msg}"
            );
        }
    }

    #[test]
    fn every_integral_scalar_converts_with_checked_range() {
        let accepted = [
            GgufValue::Uint8(7),
            GgufValue::Int8(7),
            GgufValue::Uint16(7),
            GgufValue::Int16(7),
            GgufValue::Uint32(7),
            GgufValue::Int32(7),
            GgufValue::Uint64(7),
            GgufValue::Int64(7),
        ];
        for value in &accepted {
            assert_eq!(value_to_u32(value).unwrap(), 7);
        }
        for value in [
            GgufValue::Int8(-1),
            GgufValue::Int16(-1),
            GgufValue::Int32(-1),
            GgufValue::Int64(-1),
            GgufValue::Uint64(u64::from(u32::MAX) + 1),
            GgufValue::Int64(i64::from(u32::MAX) + 1),
        ] {
            assert!(value_to_u32(&value).is_err());
        }
    }

    #[test]
    fn public_invalid_config_fails_closed_at_use() {
        let mut cfg = parse_qwen35(65, Some(TestValue::U32(1))).unwrap();
        cfg.nextn_predict_layers = cfg.block_count;
        assert!(cfg.main_block_count().is_err());
        cfg.nextn_predict_layers = cfg.block_count + 1;
        assert!(cfg.main_block_count().is_err());
    }
}
