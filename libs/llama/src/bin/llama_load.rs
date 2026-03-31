use makepad_llama::LlamaModel;

fn main() {
    let mut args = std::env::args();
    let _exe = args.next();
    let path = match args.next() {
        Some(path) => path,
        None => {
            eprintln!("usage: llama-load <model.gguf>");
            std::process::exit(2);
        }
    };

    match run(&path) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("llama-load failed: {}", err);
            std::process::exit(1);
        }
    }
}

fn run(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let model = LlamaModel::load(path)?;
    model.validate_qwen35moe_layout()?;

    println!("path: {}", model.gguf.path.display());
    println!("version: {}", model.gguf.version);
    println!("alignment: {}", model.gguf.alignment);
    println!("data_offset: {}", model.gguf.data_offset);
    println!("file_size: {}", model.gguf.file_size);
    println!("n_kv: {}", model.gguf.kv.len());
    println!("n_tensors: {}", model.gguf.tensors.len());
    println!("architecture: {}", model.architecture.name());
    if let Some(name) = &model.general.name {
        println!("general.name: {}", name);
    }
    if let Some(model_type) = &model.general.model_type {
        println!("general.type: {}", model_type);
    }
    if let Some(file_type) = model.general.file_type {
        println!("general.file_type: {}", file_type);
    }
    if let Some(qv) = model.general.quantization_version {
        println!("general.quantization_version: {}", qv);
    }

    if let Some(cfg) = &model.qwen35moe {
        println!("qwen35moe.block_count: {}", cfg.block_count);
        println!("qwen35moe.context_length: {}", cfg.context_length);
        println!("qwen35moe.embedding_length: {}", cfg.embedding_length);
        println!(
            "qwen35moe.attention.head_count: {}",
            cfg.attention_head_count
        );
        println!(
            "qwen35moe.attention.head_count_kv: {}",
            cfg.attention_head_count_kv
        );
        println!(
            "qwen35moe.rope.dimension_sections: {:?}",
            cfg.rope_dimension_sections
        );
        println!("qwen35moe.rope.freq_base: {}", cfg.rope_freq_base);
        println!("qwen35moe.expert_count: {}", cfg.expert_count);
        println!("qwen35moe.expert_used_count: {}", cfg.expert_used_count);
        println!(
            "qwen35moe.full_attention_interval: {}",
            cfg.full_attention_interval
        );
    }

    if let Some(first) = model.gguf.tensors.first() {
        println!(
            "tensor.first: {} type={} dims={:?} size_bytes={} offset={}",
            first.name,
            first.tensor_type.name(),
            first.dimensions,
            first.size_bytes,
            first.offset
        );
    }

    if let Some(last) = model.gguf.tensors.last() {
        println!(
            "tensor.last: {} type={} dims={:?} size_bytes={} offset={}",
            last.name,
            last.tensor_type.name(),
            last.dimensions,
            last.size_bytes,
            last.offset
        );
    }

    println!(
        "tensor.sample: {}",
        model.gguf.tensor_summary("blk.0.attn_qkv.weight")?
    );
    println!(
        "tensor.sample: {}",
        model.gguf.tensor_summary("blk.39.post_attention_norm.weight")?
    );

    Ok(())
}
