use super::{
    bf16_word_to_f32, fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxDType,
    MlxFamilySnapshot, MlxIndexedSafetensors, MlxModelFamily, MlxModelManifest, MlxModelSnapshot,
    MlxQwen35MoeIndexedSafetensors, MlxQwen35MoeLayerKind, MlxQwen35MoeSnapshot,
    MlxQwen35MoeTensors, MlxSafetensorsHeader, MlxTokenizer, GEMMA4_QPROJ_CASE_INNER_DIM,
    GEMMA4_QPROJ_CASE_OUTPUT_DIM, GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64,
};
use crate::chat::{MlxChatDecodeMode, MlxChatRole, MlxChatSession};
use crate::{
    extract_qwen35moe_assistant_response_text, format_qwen35moe_chat_prompt_with_options,
    MlxModelLayerRole, MlxQwen35MoeRuntimeSession, QwenChatMessage, QwenChatPromptOptions,
    QwenChatRole, QwenThinkingMode,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn local_model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/models/gemma-4-26b-mlx")
}

fn local_model_shard_1() -> PathBuf {
    local_model_dir().join("model-00001-of-00003.safetensors")
}

fn temp_model_dir(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("makepad_mlx_{prefix}_{stamp}"));
    fs::create_dir_all(&path).unwrap();
    path
}

fn write_text(path: &PathBuf, text: &str) {
    fs::write(path, text).unwrap();
}

fn write_fake_safetensors(path: &PathBuf, names: &[String]) {
    let mut header = String::from("{\"__metadata__\":{\"format\":\"mlx\"}");
    let mut payload = Vec::with_capacity(names.len() * 2);
    let mut offset = 0u64;
    for name in names {
        header.push_str(&format!(
            ",\"{}\":{{\"dtype\":\"BF16\",\"shape\":[1],\"data_offsets\":[{},{}]}}",
            name,
            offset,
            offset + 2
        ));
        payload.extend_from_slice(&[0u8, 0u8]);
        offset += 2;
    }
    header.push('}');

    let mut bytes = Vec::with_capacity(8 + header.len() + payload.len());
    bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(&payload);
    fs::write(path, bytes).unwrap();
}

fn write_qwen35moe_test_model(root: &PathBuf) {
    write_text(
        &root.join("config.json"),
        r#"{
                "architectures":["Qwen3_5MoeForConditionalGeneration"],
                "image_token_id":9,
                "model_type":"qwen3_5_moe",
                "quantization":{
                    "bits":4,
                    "group_size":64,
                    "mode":"affine",
                    "model.language_model.layers.0.linear_attn.in_proj_qkv":{
                        "bits":8,
                        "group_size":64,
                        "mode":"affine"
                    }
                },
                "text_config":{
                    "attention_bias":false,
                    "attention_dropout":0.0,
                    "attn_output_gate":true,
                    "bos_token_id":1,
                    "dtype":"bfloat16",
                    "eos_token_id":2,
                    "full_attention_interval":4,
                    "head_dim":256,
                    "hidden_act":"silu",
                    "hidden_size":2048,
                    "initializer_range":0.02,
                    "layer_types":["linear_attention","linear_attention","linear_attention","full_attention"],
                    "linear_conv_kernel_dim":4,
                    "linear_key_head_dim":128,
                    "linear_num_key_heads":16,
                    "linear_num_value_heads":32,
                    "linear_value_head_dim":128,
                    "mamba_ssm_dtype":"float32",
                    "max_position_embeddings":262144,
                    "model_type":"qwen3_5_moe_text",
                    "moe_intermediate_size":512,
                    "mtp_num_hidden_layers":1,
                    "mtp_use_dedicated_embeddings":false,
                    "num_attention_heads":16,
                    "num_experts":256,
                    "num_experts_per_tok":8,
                    "num_hidden_layers":4,
                    "num_key_value_heads":2,
                    "output_router_logits":false,
                    "pad_token_id":null,
                    "partial_rotary_factor":0.25,
                    "rms_norm_eps":0.000001,
                    "rope_parameters":{
                        "mrope_interleaved":true,
                        "mrope_section":[11,11,10],
                        "partial_rotary_factor":0.25,
                        "rope_theta":10000000.0,
                        "rope_type":"default"
                    },
                    "router_aux_loss_coef":0.001,
                    "shared_expert_intermediate_size":512,
                    "tie_word_embeddings":false,
                    "use_cache":true,
                    "vocab_size":248320
                },
                "tie_word_embeddings":false,
                "transformers_version":"4.52.3",
                "video_token_id":10,
                "vision_config":{
                    "deepstack_visual_indexes":[],
                    "depth":1,
                    "hidden_act":"gelu_pytorch_tanh",
                    "hidden_size":1152,
                    "in_channels":3,
                    "initializer_range":0.02,
                    "intermediate_size":4304,
                    "model_type":"qwen3_5_moe",
                    "num_heads":16,
                    "num_position_embeddings":2304,
                    "out_hidden_size":2048,
                    "patch_size":16,
                    "spatial_merge_size":2,
                    "temporal_patch_size":2
                },
                "vision_end_token_id":11,
                "vision_start_token_id":12
            }"#,
    );
    write_text(
        &root.join("generation_config.json"),
        r#"{
                "bos_token_id":1,
                "do_sample":true,
                "eos_token_id":[2],
                "pad_token_id":1,
                "temperature":1.0,
                "top_k":20,
                "top_p":0.95,
                "transformers_version":"4.52.3"
            }"#,
    );
    write_text(
        &root.join("preprocessor_config.json"),
        r#"{
                "size":{"longest_edge":16777216,"shortest_edge":65536},
                "patch_size":16,
                "temporal_patch_size":2,
                "merge_size":2,
                "image_mean":[0.5,0.5,0.5],
                "image_std":[0.5,0.5,0.5],
                "processor_class":"Qwen3VLProcessor",
                "image_processor_type":"Qwen2VLImageProcessorFast"
            }"#,
    );
    write_text(
        &root.join("tokenizer_config.json"),
        r#"{
                "tokenizer_class":"Qwen2Tokenizer",
                "model_max_length":262144,
                "extra_special_tokens":{
                    "image_token":"<|image_pad|>",
                    "vision_bos_token":"<|vision_start|>",
                    "vision_eos_token":"<|vision_end|>"
                }
            }"#,
    );
    write_text(
        &root.join("tokenizer.json"),
        r#"{
                "normalizer":{"type":"NFC"},
                "pre_tokenizer":{"type":"Sequence"},
                "decoder":{"type":"ByteLevel"},
                "model":{
                    "type":"BPE",
                    "byte_fallback":false,
                    "vocab":{"Ġ":0,"h":1,"i":2,"Ġh":3,"Ġhi":4},
                    "merges":["Ġ h","Ġh i"]
                },
                "added_tokens":[{"id":5,"content":"<|image_pad|>","special":true}]
            }"#,
    );

    let mut names = vec![
        "lm_head.weight".to_string(),
        "model.language_model.embed_tokens.weight".to_string(),
        "model.language_model.norm.weight".to_string(),
        "model.visual.patch_embed.proj.weight".to_string(),
        "model.visual.patch_embed.proj.bias".to_string(),
        "model.visual.pos_embed.weight".to_string(),
        "model.visual.blocks.0.norm1.weight".to_string(),
        "model.visual.blocks.0.norm1.bias".to_string(),
        "model.visual.blocks.0.attn.qkv.weight".to_string(),
        "model.visual.blocks.0.attn.qkv.bias".to_string(),
        "model.visual.blocks.0.attn.proj.weight".to_string(),
        "model.visual.blocks.0.attn.proj.bias".to_string(),
        "model.visual.blocks.0.norm2.weight".to_string(),
        "model.visual.blocks.0.norm2.bias".to_string(),
        "model.visual.blocks.0.mlp.linear_fc1.weight".to_string(),
        "model.visual.blocks.0.mlp.linear_fc1.bias".to_string(),
        "model.visual.blocks.0.mlp.linear_fc2.weight".to_string(),
        "model.visual.blocks.0.mlp.linear_fc2.bias".to_string(),
        "model.visual.merger.norm.weight".to_string(),
        "model.visual.merger.norm.bias".to_string(),
        "model.visual.merger.linear_fc1.weight".to_string(),
        "model.visual.merger.linear_fc1.bias".to_string(),
        "model.visual.merger.linear_fc2.weight".to_string(),
        "model.visual.merger.linear_fc2.bias".to_string(),
    ];
    for layer in 0..4u32 {
        names.push(format!(
            "model.language_model.layers.{layer}.input_layernorm.weight"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.post_attention_layernorm.weight"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.mlp.gate.weight"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.mlp.experts.gate_up_proj"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.mlp.experts.down_proj"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.mlp.shared_expert_gate.weight"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.mlp.shared_expert.gate_proj.weight"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.mlp.shared_expert.up_proj.weight"
        ));
        names.push(format!(
            "model.language_model.layers.{layer}.mlp.shared_expert.down_proj.weight"
        ));
        if layer == 3 {
            names.push(format!(
                "model.language_model.layers.{layer}.self_attn.q_proj.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.self_attn.k_proj.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.self_attn.v_proj.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.self_attn.o_proj.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.self_attn.q_norm.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.self_attn.k_norm.weight"
            ));
        } else {
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.in_proj_qkv.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.in_proj_z.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.conv1d.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.dt_bias"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.A_log"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.in_proj_a.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.in_proj_b.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.norm.weight"
            ));
            names.push(format!(
                "model.language_model.layers.{layer}.linear_attn.out_proj.weight"
            ));
        }
    }
    names.sort();

    let mut weight_index = String::from("{\"metadata\":{\"total_size\":1},\"weight_map\":{");
    for (index, name) in names.iter().enumerate() {
        if index != 0 {
            weight_index.push(',');
        }
        weight_index.push_str(&format!(
            "\"{}\":\"model-00001-of-00001.safetensors\"",
            name
        ));
    }
    weight_index.push_str("}}");
    write_text(&root.join("model.safetensors.index.json"), &weight_index);
    write_fake_safetensors(&root.join("model-00001-of-00001.safetensors"), &names);
}

#[test]
fn loads_local_gemma4_mlx_snapshot() {
    let snapshot = MlxModelSnapshot::load(local_model_dir()).unwrap();

    assert_eq!(
        snapshot.config.architectures,
        vec!["Gemma4ForConditionalGeneration".to_string()]
    );
    assert_eq!(snapshot.config.model_type, "gemma4");
    assert_eq!(snapshot.config.text_config.num_hidden_layers, 30);
    assert_eq!(snapshot.config.vision_config.num_hidden_layers, 27);
    assert_eq!(snapshot.config.quantization.bits, 4);
    assert_eq!(snapshot.config.quantization.group_size, 64);
    assert_eq!(snapshot.config.quantization.mode, "affine");
    assert_eq!(snapshot.processor_config.image_processor.size.height, 224);
    assert_eq!(snapshot.processor_config.image_processor.size.width, 224);
    assert_eq!(
        snapshot.tokenizer_config.model_max_length,
        1000000000000000019884624838656u128
    );
    assert_eq!(snapshot.weight_index.metadata.total_size, 15_335_574_684);
    assert_eq!(snapshot.unique_weight_shards().len(), 3);
    assert_eq!(
        snapshot
            .weight_index
            .weight_map
            .get("language_model.model.layers.0.self_attn.q_proj.weight")
            .map(String::as_str),
        Some("model-00001-of-00003.safetensors")
    );
    assert_eq!(
        snapshot
            .weight_index
            .weight_map
            .get("embed_vision.embedding_projection.weight")
            .map(String::as_str),
        Some("model-00003-of-00003.safetensors")
    );
}

#[test]
fn reads_local_safetensors_header_without_touching_payload() {
    let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();

    assert_eq!(header.header_len, 64_065);
    assert_eq!(
        header.metadata.get("format").map(String::as_str),
        Some("mlx")
    );
    assert_eq!(header.tensors.len(), 488);

    let embed_weight = header
        .tensor("language_model.model.embed_tokens.weight")
        .unwrap();
    assert_eq!(embed_weight.dtype, MlxDType::U32);
    assert_eq!(embed_weight.shape, vec![262_144, 352]);
    assert_eq!(embed_weight.data_offsets, [3_116_339_724, 3_485_438_476]);
    assert_eq!(
        embed_weight.data_len_bytes(),
        embed_weight.expected_len_bytes()
    );

    let q_proj = header
        .tensor("language_model.model.layers.0.self_attn.q_proj.weight")
        .unwrap();
    assert_eq!(q_proj.dtype, MlxDType::U32);
    assert_eq!(q_proj.shape, vec![4_096, 352]);
    assert_eq!(q_proj.data_offsets, [3_612_518_924, 3_618_286_092]);
    assert_eq!(q_proj.data_len_bytes(), q_proj.expected_len_bytes());

    let q_proj_scales = header
        .tensor("language_model.model.layers.0.self_attn.q_proj.scales")
        .unwrap();
    assert_eq!(q_proj_scales.dtype, MlxDType::BF16);
    assert_eq!(q_proj_scales.shape, vec![4_096, 44]);
    assert_eq!(q_proj_scales.data_offsets, [2_536_973_576, 2_537_334_024]);
    assert_eq!(
        q_proj_scales.file_offsets(header.payload_base_offset())[0],
        2_537_037_649
    );
}

#[test]
fn indexed_safetensors_resolves_late_layer_to_correct_shard() {
    let indexed = MlxIndexedSafetensors::load(local_model_dir()).unwrap();

    assert_eq!(
        indexed
            .shard_name_for_tensor("language_model.model.layers.29.self_attn.q_proj.weight")
            .unwrap(),
        "model-00003-of-00003.safetensors"
    );

    let header = indexed
        .header_for_tensor("language_model.model.layers.29.self_attn.q_proj.weight")
        .unwrap();
    assert!(header.path.ends_with("model-00003-of-00003.safetensors"));

    let entry = indexed
        .tensor("language_model.model.layers.29.self_attn.q_proj.weight")
        .unwrap();
    assert_eq!(entry.dtype, MlxDType::U32);
    assert_eq!(entry.shape, vec![8_192, 352]);
}

#[test]
fn loads_local_tokenizer_metadata() {
    let tokenizer = MlxTokenizer::load(local_model_dir()).unwrap();

    assert!(tokenizer.vocab_size() > 260_000);
    assert!(tokenizer.merge_count() > 500_000);
    assert_eq!(tokenizer.token_to_id("<bos>"), Some(2));
    assert_eq!(tokenizer.token_to_id("<eos>"), Some(1));
    assert_eq!(tokenizer.token_to_id("<|video|>"), Some(258_884));
    assert_eq!(tokenizer.token_to_id("say"), Some(30_468));
    assert_eq!(tokenizer.token_to_id("▁hi"), Some(5_631));
    assert_eq!(tokenizer.token_to_id("▁("), Some(568));
    assert_eq!(tokenizer.id_to_token(2), Some("<bos>"));
    assert_eq!(tokenizer.id_to_token(568), Some("▁("));
}

#[test]
fn loads_qwen_manifest_with_preprocessor_fallback() {
    let root = temp_model_dir("qwen_manifest");
    write_text(&root.join("config.json"), r#"{"model_type":"qwen3_5_moe"}"#);
    write_text(&root.join("generation_config.json"), "{}");
    write_text(&root.join("preprocessor_config.json"), "{}");
    write_text(
        &root.join("tokenizer_config.json"),
        r#"{
                "tokenizer_class":"Qwen2Tokenizer",
                "model_max_length":262144,
                "extra_special_tokens":{
                    "image_token":"<|image_pad|>",
                    "vision_bos_token":"<|vision_start|>",
                    "vision_eos_token":"<|vision_end|>"
                }
            }"#,
    );
    write_text(
        &root.join("tokenizer.json"),
        r#"{
                "normalizer":{"type":"NFC"},
                "pre_tokenizer":{"type":"Sequence"},
                "decoder":{"type":"ByteLevel"},
                "model":{
                    "type":"BPE",
                    "byte_fallback":false,
                    "vocab":{"Ġ":0,"h":1,"i":2,"Ġh":3,"Ġhi":4},
                    "merges":["Ġ h","Ġh i"]
                },
                "added_tokens":[{"id":5,"content":"<|image_pad|>","special":true}]
            }"#,
    );
    write_text(
        &root.join("model.safetensors.index.json"),
        r#"{"metadata":{"total_size":71903645408.0},"weight_map":{}}"#,
    );

    let manifest = MlxModelManifest::load(&root).unwrap();
    assert_eq!(manifest.family, MlxModelFamily::Qwen35Moe);
    assert!(manifest
        .paths
        .processor_config_json
        .as_ref()
        .unwrap()
        .ends_with("preprocessor_config.json"));
    assert_eq!(manifest.tokenizer_config.image_token, "<|image_pad|>");
    assert_eq!(manifest.tokenizer_config.boi_token, "<|vision_start|>");
    assert_eq!(manifest.tokenizer_config.eoi_token, "<|vision_end|>");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen_bytelevel_tokenizer_roundtrips_minimal_prompt() {
    let root = temp_model_dir("qwen_tokenizer");
    write_text(&root.join("config.json"), r#"{"model_type":"qwen3_5_moe"}"#);
    write_text(&root.join("generation_config.json"), "{}");
    write_text(&root.join("preprocessor_config.json"), "{}");
    write_text(
        &root.join("tokenizer_config.json"),
        r#"{
                "tokenizer_class":"Qwen2Tokenizer",
                "model_max_length":262144,
                "extra_special_tokens":{"image_token":"<|image_pad|>"}
            }"#,
    );
    write_text(
        &root.join("tokenizer.json"),
        r#"{
                "normalizer":{"type":"NFC"},
                "pre_tokenizer":{"type":"Sequence"},
                "decoder":{"type":"ByteLevel"},
                "model":{
                    "type":"BPE",
                    "byte_fallback":false,
                    "vocab":{"Ġ":0,"h":1,"i":2,"Ġh":3,"Ġhi":4},
                    "merges":["Ġ h","Ġh i"]
                },
                "added_tokens":[{"id":5,"content":"<|image_pad|>","special":true}]
            }"#,
    );
    write_text(
        &root.join("model.safetensors.index.json"),
        r#"{"metadata":{"total_size":1},"weight_map":{}}"#,
    );

    let tokenizer = MlxTokenizer::load(&root).unwrap();
    assert_eq!(tokenizer.encode(" hi").unwrap(), vec![4]);
    assert_eq!(tokenizer.decode(&[4]).unwrap(), " hi");
    assert_eq!(tokenizer.encode("<|image_pad|>").unwrap(), vec![5]);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen35moe_snapshot_builds_canonical_tensor_map() {
    let root = temp_model_dir("qwen_snapshot");
    write_qwen35moe_test_model(&root);

    let snapshot = MlxQwen35MoeSnapshot::load(&root).unwrap();
    assert_eq!(
        snapshot.layer_kind(0).unwrap(),
        MlxQwen35MoeLayerKind::Recurrent
    );
    assert_eq!(
        snapshot.layer_kind(3).unwrap(),
        MlxQwen35MoeLayerKind::Attention
    );

    let aliases = snapshot.canonical_weight_map().unwrap();
    assert_eq!(
        aliases.get("token_embd.weight").map(String::as_str),
        Some("model.language_model.embed_tokens.weight")
    );
    assert_eq!(
        aliases.get("blk.0.attn_qkv.weight").map(String::as_str),
        Some("model.language_model.layers.0.linear_attn.in_proj_qkv.weight")
    );
    assert_eq!(
        aliases.get("blk.3.attn_q.weight").map(String::as_str),
        Some("model.language_model.layers.3.self_attn.q_proj.weight")
    );
    assert_eq!(
        aliases
            .get("blk.0.ffn_gate_up_exps.weight")
            .map(String::as_str),
        Some("model.language_model.layers.0.mlp.experts.gate_up_proj")
    );
    assert_eq!(
        aliases
            .get("visual.blk.0.attn_qkv.weight")
            .map(String::as_str),
        Some("model.visual.blocks.0.attn.qkv.weight")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen35moe_indexed_safetensors_resolves_canonical_names() {
    let root = temp_model_dir("qwen_indexed");
    write_qwen35moe_test_model(&root);

    let indexed = MlxQwen35MoeIndexedSafetensors::load(&root).unwrap();
    assert_eq!(
        indexed.actual_tensor_name("blk.3.attn_k.weight").unwrap(),
        "model.language_model.layers.3.self_attn.k_proj.weight"
    );
    assert_eq!(
        indexed
            .shard_name_for_tensor("visual.merger.fc1.weight")
            .unwrap(),
        "model-00001-of-00001.safetensors"
    );
    let tensor = indexed.tensor("blk.0.ssm_out.weight").unwrap();
    assert_eq!(tensor.dtype, MlxDType::BF16);
    assert_eq!(tensor.shape, vec![1]);
    assert_eq!(
        indexed
            .quantization_for_tensor("blk.0.attn_qkv.weight")
            .unwrap()
            .unwrap()
            .bits,
        8
    );
    assert_eq!(
        indexed
            .quantization_for_tensor("blk.0.ssm_out.weight")
            .unwrap()
            .unwrap()
            .bits,
        4
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen35moe_indexed_builds_typed_text_inventory() {
    let root = temp_model_dir("qwen_inventory");
    write_qwen35moe_test_model(&root);

    let indexed = MlxQwen35MoeIndexedSafetensors::load(&root).unwrap();
    let tensors = MlxQwen35MoeTensors::from_indexed(&indexed).unwrap();
    assert_eq!(tensors.globals.token_embd, "token_embd.weight");
    assert_eq!(tensors.layers.len(), 4);
    assert_eq!(tensors.layers[0].kind, MlxQwen35MoeLayerKind::Recurrent);
    assert!(tensors.layers[0].attention.is_none());
    assert!(tensors.layers[0].recurrent.is_some());
    assert!(tensors.layers[0].moe.uses_merged_gate_up());
    assert_eq!(tensors.layers[3].kind, MlxQwen35MoeLayerKind::Attention);
    assert!(tensors.layers[3].attention.is_some());
    assert!(tensors.layers[3].recurrent.is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn family_snapshot_front_door_loads_qwen_model() {
    let root = temp_model_dir("qwen_front_door");
    write_qwen35moe_test_model(&root);

    let family = MlxFamilySnapshot::load(&root).unwrap();
    assert_eq!(family.family(), MlxModelFamily::Qwen35Moe);
    match family {
        MlxFamilySnapshot::Gemma4(_) => panic!("expected qwen family snapshot"),
        MlxFamilySnapshot::Qwen35Moe(snapshot) => {
            assert_eq!(snapshot.config.model_type, "qwen3_5_moe");
        }
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen35moe_runtime_session_loads_dims_and_cache_template() {
    let root = temp_model_dir("qwen_runtime");
    write_qwen35moe_test_model(&root);

    let session = MlxQwen35MoeRuntimeSession::load(&root).unwrap();
    assert_eq!(session.dims.vocab_size, 1);
    assert_eq!(session.dims.block_count, 4);
    assert_eq!(session.dims.embedding_length, 2048);
    assert_eq!(session.dims.attention_head_count, 16);
    assert_eq!(session.dims.attention_head_count_kv, 2);
    assert_eq!(session.dims.ssm_state_size, 128);
    assert_eq!(session.dims.ssm_group_count, 16);
    assert_eq!(session.dims.ssm_time_step_rank, 32);
    assert_eq!(session.dims.recurrent_value_head_dim().unwrap(), 128);
    assert_eq!(session.cache_template.attention_layers, vec![3]);
    assert_eq!(session.cache_template.recurrent_layers, vec![0, 1, 2]);
    assert_eq!(session.cache_template.attention_k_width, 512);
    assert_eq!(session.cache_template.attention_v_width, 512);
    assert_eq!(session.cache_template.recurrent_r_width, 24576);
    assert_eq!(session.cache_template.recurrent_s_width, 524288);
    assert_eq!(
        session.stop_tokens.iter().copied().collect::<Vec<_>>(),
        vec![2]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen35moe_runtime_session_builds_decode_state_layout() {
    let root = temp_model_dir("qwen_decode_state");
    write_qwen35moe_test_model(&root);

    let session = MlxQwen35MoeRuntimeSession::load(&root).unwrap();
    let decode_state = session.new_decode_state().unwrap();
    assert_eq!(decode_state.token_count, 0);
    assert_eq!(decode_state.layers.len(), 4);
    match &decode_state.layers[0] {
        crate::MlxQwen35MoeLayerDecodeState::Recurrent(layer) => {
            assert_eq!(layer.layer_index, 0);
            assert_eq!(layer.conv_state.len(), 24_576);
            assert_eq!(layer.ssm_state.len(), 524_288);
        }
        other => panic!("expected recurrent layer state, got {other:?}"),
    }
    match &decode_state.layers[3] {
        crate::MlxQwen35MoeLayerDecodeState::Attention(layer) => {
            assert_eq!(layer.layer_index, 3);
            assert!(layer.key_cache.is_empty());
            assert!(layer.value_cache.is_empty());
        }
        other => panic!("expected attention layer state, got {other:?}"),
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen35moe_chat_prompt_and_extractors_match_expected_control_tokens() {
    let root = temp_model_dir("qwen_prompt");
    write_qwen35moe_test_model(&root);

    let session = MlxQwen35MoeRuntimeSession::load(&root).unwrap();
    let prompt = session
        .format_chat_prompt(
            &[
                QwenChatMessage::new(QwenChatRole::System, "You are terse."),
                QwenChatMessage::new(QwenChatRole::User, "Describe the duck."),
            ],
            true,
        )
        .unwrap();
    assert!(prompt.contains("<|im_start|>system\nYou are terse.<|im_end|>\n"));
    assert!(prompt.contains("<|im_start|>user\n<|vision_start|><|image_pad|><|vision_end|>Describe the duck.<|im_end|>\n"));
    assert!(prompt.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));

    let think_prompt = format_qwen35moe_chat_prompt_with_options(
        session.tokenizer_config(),
        &[
            QwenChatMessage::new(QwenChatRole::System, "You are terse."),
            QwenChatMessage::new(QwenChatRole::User, "Describe the duck."),
        ],
        QwenChatPromptOptions {
            thinking_mode: QwenThinkingMode::Enabled,
            ..QwenChatPromptOptions::default()
        },
    )
    .unwrap();
    assert!(think_prompt.ends_with("<|im_start|>assistant\n<think>\n"));

    let text = extract_qwen35moe_assistant_response_text(
        session.tokenizer_config(),
        "<|im_start|>assistant\n<think>\ninternal\n</think>\nFinal answer<|im_end|>",
    );
    assert_eq!(text, "Final answer");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mlx_chat_session_routes_qwen_without_gemma_specific_cli_logic() {
    let root = temp_model_dir("qwen_generic_chat");
    write_qwen35moe_test_model(&root);

    let mut session =
        MlxChatSession::load_with_mode(&root, Some(32), MlxChatDecodeMode::Greedy).unwrap();
    assert_eq!(session.family(), MlxModelFamily::Qwen35Moe);
    assert_eq!(session.decode_mode(), MlxChatDecodeMode::Greedy);
    assert!(session.backend_label().starts_with("qwen-"));
    assert!(session.messages().is_empty());

    match session.send_user_message_streaming("Describe the duck.", |_| Ok(())) {
        Ok(output) => {
            assert!(!output.generated_token_ids.is_empty());
            assert_eq!(session.messages().len(), 2);
            assert_eq!(session.messages()[0].role, MlxChatRole::User);
            assert_eq!(session.messages()[1].role, MlxChatRole::Assistant);
        }
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("missing tokenizer piece"),
                "{message}"
            );
            assert!(session.messages().is_empty());
        }
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn qwen35moe_execution_plan_builds_tensor_inventory() {
    let root = temp_model_dir("qwen_execution_plan");
    write_qwen35moe_test_model(&root);

    let session = MlxQwen35MoeRuntimeSession::load(&root).unwrap();
    let plan = session.execution_plan().unwrap();

    assert_eq!(plan.layer_count(), 4);
    assert_eq!(
        plan.inventory
            .count_layers_with_role(MlxModelLayerRole::Attention),
        1
    );
    assert_eq!(
        plan.inventory
            .count_layers_with_role(MlxModelLayerRole::Recurrent),
        3
    );
    assert_eq!(plan.inventory.unique_tensor_count(), 72);
    assert_eq!(plan.inventory.total_tensor_bytes(), 144);
    assert_eq!(plan.tail_probe.output.actual_name, "lm_head.weight");
    assert_eq!(
        plan.tail_probe.output_norm.actual_name,
        "model.language_model.norm.weight"
    );
    assert_eq!(
        plan.inventory.globals["token_embd"].actual_name,
        "model.language_model.embed_tokens.weight"
    );
    assert_eq!(plan.inventory.layers[0].role, MlxModelLayerRole::Recurrent);
    assert_eq!(plan.inventory.layers[3].role, MlxModelLayerRole::Attention);
    assert_eq!(
        plan.inventory.layers[0].tensors["ssm_conv1d"].actual_name,
        "model.language_model.layers.0.linear_attn.conv1d.weight"
    );
    assert_eq!(
        plan.inventory.layers[3].tensors["attn_q"].actual_name,
        "model.language_model.layers.3.self_attn.q_proj.weight"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tokenizer_encodes_and_decodes_simple_phrase() {
    let tokenizer = MlxTokenizer::load(local_model_dir()).unwrap();

    assert_eq!(tokenizer.encode("say hi").unwrap(), vec![30_468, 5_631]);
    assert_eq!(tokenizer.encode(" hi").unwrap(), vec![5_631]);
    assert_eq!(tokenizer.decode(&[30_468, 5_631]).unwrap(), "say hi");
    assert_eq!(tokenizer.decode(&[1_879, 5_631]).unwrap(), " say hi");
}

#[test]
fn embeds_and_norms_local_text_token_rows() {
    let weights = MlxIndexedSafetensors::load(local_model_dir()).unwrap();

    let embed = weights.embed_token_bf16_words(30_468).unwrap();
    assert_eq!(embed.len(), 2_816);

    let final_norm = weights.final_text_norm_bf16_words(&embed).unwrap();
    assert_eq!(final_norm.len(), 2_816);
}

#[test]
#[ignore]
fn embed_rows_report_hashes_for_two_token_prompt() {
    let weights = MlxIndexedSafetensors::load(local_model_dir()).unwrap();
    for token_id in [30_468u32, 5_631u32] {
        let bits = weights
            .embed_token_bf16_words(token_id)
            .unwrap()
            .into_iter()
            .map(|word| (word as u32) << 16)
            .collect::<Vec<_>>();
        println!(
            "token_id={} embed_fnv1a64=0x{:016X}",
            token_id,
            fnv1a64_u32_words(&bits)
        );
        println!(
            "token_id={} embed_first16_f32_bits={}",
            token_id,
            bits.iter()
                .take(16)
                .map(|bits| format!("0x{bits:08X}"))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
}

#[test]
fn reads_local_tensor_payload_words() {
    let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();

    let q_proj_weight = header
        .read_u32_tensor_words("language_model.model.layers.0.self_attn.q_proj.weight")
        .unwrap();
    assert_eq!(
        &q_proj_weight[..8],
        &[
            2_259_126_473,
            1_283_001_501,
            2_291_701_430,
            1_970_953_151,
            1_283_929_482,
            2_027_333_543,
            934_918_473,
            3_033_893_010,
        ]
    );

    let q_proj_scales = header
        .read_bf16_tensor_words("language_model.model.layers.0.self_attn.q_proj.scales")
        .unwrap();
    assert_eq!(
        &q_proj_scales[..16],
        &[
            15_321, 48_110, 48_135, 48_112, 15_290, 48_057, 15_308, 15_307, 15_254, 15_260, 15_397,
            15_275, 15_300, 15_297, 48_099, 15_299,
        ]
    );
}

#[test]
fn dequantizes_one_local_q_proj_row_matches_mlx_oracle() {
    let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
    let row = header
        .affine_dequantize_row_f32(
            "language_model.model.layers.0.self_attn.q_proj.weight",
            "language_model.model.layers.0.self_attn.q_proj.scales",
            "language_model.model.layers.0.self_attn.q_proj.biases",
            0,
            64,
            4,
        )
        .unwrap();
    assert_eq!(row.len(), 2_816);
    assert_eq!(
        fnv1a64_u32_words(&row.iter().map(|value| value.to_bits()).collect::<Vec<_>>()),
        0x2D44_4223_7EE7_C10F
    );
    assert_eq!(
        &row[..16]
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        &[
            0x3BD9_0000,
            0x3CD9_0000,
            0x0000_0000,
            0x0000_0000,
            0xBBD9_0000,
            0x3C59_0000,
            0xBC59_0000,
            0x0000_0000,
            0x3D08_0000,
            0x3BD9_0000,
            0x3CD9_0000,
            0xBD59_0000,
            0x3BD9_0000,
            0xBBD9_0000,
            0x3CD9_0000,
            0xBCD9_0000,
        ]
    );
}

#[test]
fn quantized_matmul_one_local_q_proj_case_matches_mlx_oracle() {
    let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
    let x = gemma4_qproj_case_input_bf16_words(GEMMA4_QPROJ_CASE_INNER_DIM);
    assert_eq!(
        x[..16]
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .map(f32::to_bits)
            .collect::<Vec<_>>(),
        vec![
            0xBF80_0000,
            0xBF40_0000,
            0xBF00_0000,
            0xBE80_0000,
            0x0000_0000,
            0x3E80_0000,
            0x3F00_0000,
            0x3F40_0000,
            0x3F80_0000,
            0x3F00_0000,
            0x0000_0000,
            0xBF00_0000,
            0xBF80_0000,
            0x3E00_0000,
            0x3EC0_0000,
            0x3F20_0000,
        ]
    );

    let out = header
        .affine_quantized_matmul_t_f32(
            &x,
            "language_model.model.layers.0.self_attn.q_proj.weight",
            "language_model.model.layers.0.self_attn.q_proj.scales",
            "language_model.model.layers.0.self_attn.q_proj.biases",
            64,
            4,
        )
        .unwrap();
    assert_eq!(out.len(), GEMMA4_QPROJ_CASE_OUTPUT_DIM);
    let out_bits = out.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        &out_bits[..16],
        &[
            0xBF59_0000,
            0x4029_0000,
            0x3F3B_0000,
            0xBF63_0000,
            0x3DAF_0000,
            0xBF51_0000,
            0xBF49_0000,
            0x3FCE_0000,
            0x3D8B_0000,
            0xBEB9_0000,
            0x3F0F_0000,
            0x3E8E_0000,
            0x3DF2_0000,
            0x3E80_0000,
            0x3E89_0000,
            0xC022_0000,
        ]
    );
    assert_eq!(
        fnv1a64_u32_words(&out_bits),
        GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64
    );
}

#[test]
fn rms_norm_one_local_layer0_input_case_matches_mlx_gpu_oracle() {
    let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
    let x = gemma4_qproj_case_input_bf16_words(2_816);
    let out = header
        .rms_norm_weighted_f32(
            &x,
            "language_model.model.layers.0.input_layernorm.weight",
            1e-6,
        )
        .unwrap();
    assert_eq!(out.len(), 2_816);
    let out_bits = out.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        &out_bits[..16],
        &[
            0xC0A2_0000,
            0xC080_0000,
            0xC033_0000,
            0xBFBC_0000,
            0x0000_0000,
            0x3FB6_0000,
            0x402F_0000,
            0x40E3_0000,
            0x4126_0000,
            0x4041_0000,
            0x0000_0000,
            0xC048_0000,
            0xC11C_0000,
            0x3F6C_0000,
            0x4081_0000,
            0x40A4_0000,
        ]
    );
    assert_eq!(fnv1a64_u32_words(&out_bits), 0xBF5E_A05B_53DF_E923);
}
