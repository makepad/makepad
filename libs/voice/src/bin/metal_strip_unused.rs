use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

const METAL_PATH: &str = "libs/voice/src/metal/ggml/ggml-metal.metal";
const METAL_BACKUP_PATH: &str = "libs/voice/src/metal/ggml/ggml-metal.metal.bak";
const COMMON_H_PATH: &str = "libs/voice/src/metal/ggml/ggml-common.h";
const METAL_IMPL_H_PATH: &str = "libs/voice/src/metal/ggml/ggml-metal-impl.h";
const REMOVED_REPORT_PATH: &str = "/tmp/metal_strip_removed_kernels.txt";
const TURBO_KERNEL_KEEP: &[&str] = &[
    "kernel_unary_f32_f32_4",
    "kernel_bin_fuse_f32_f32_f32_4",
    "kernel_cpy_f32_f16",
    "kernel_im2col_f32",
    "kernel_mul_mm_f16_f32",
    "kernel_mul_mv_ext_f16_f32_r1_2",
    "kernel_mul_mv_ext_f16_f32_r1_3",
    "kernel_mul_mv_ext_f16_f32_r1_4",
    "kernel_mul_mv_ext_f16_f32_r1_5",
    "kernel_mul_mv_ext_f32_f32_r1_2",
    "kernel_mul_mv_ext_f32_f32_r1_3",
    "kernel_mul_mv_ext_f32_f32_r1_4",
    "kernel_mul_mv_ext_f32_f32_r1_5",
    "kernel_mul_mv_f16_f32_4",
    "kernel_mul_mv_f32_f32_4",
    "kernel_norm_mul_add_f32_4",
    "kernel_flash_attn_ext_pad",
    "kernel_flash_attn_ext_blk",
    "kernel_flash_attn_ext_vec_reduce",
    "kernel_flash_attn_ext_f16_dk64_dv64",
    "kernel_flash_attn_ext_vec_f32_dk64_dv64",
];

#[derive(Clone, Debug)]
struct KernelBlock {
    start: usize,
    end: usize,
    name: String,
}

#[derive(Clone, Debug)]
struct TableBlock {
    start: usize,
    end: usize,
    name: String,
}

fn line_start(src: &str, idx: usize) -> usize {
    src[..idx].rfind('\n').map(|p| p + 1).unwrap_or(0)
}

fn prev_line_start(src: &str, current_start: usize) -> Option<usize> {
    if current_start == 0 {
        return None;
    }
    let prev_end = current_start.saturating_sub(1);
    Some(src[..prev_end].rfind('\n').map(|p| p + 1).unwrap_or(0))
}

fn line_end(src: &str, start: usize) -> usize {
    src[start..]
        .find('\n')
        .map(|o| start + o)
        .unwrap_or(src.len())
}

fn find_matching_brace(src: &str, open_idx: usize) -> Option<usize> {
    let b = src.as_bytes();
    let mut i = open_idx;
    let mut depth = 0usize;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn parse_export_name(block_src: &str) -> String {
    if let Some(pos) = block_src.find("host_name(\"") {
        let start = pos + "host_name(\"".len();
        if let Some(end_rel) = block_src[start..].find('"') {
            return block_src[start..start + end_rel].to_string();
        }
    }

    if let Some(pos) = block_src.find("kernel void") {
        let mut s = &block_src[pos + "kernel void".len()..];
        s = s.trim_start();
        let mut out = String::new();
        for ch in s.chars() {
            if ch.is_whitespace() || ch == '(' || ch == '<' {
                break;
            }
            out.push(ch);
        }
        if !out.is_empty() {
            return out;
        }
    }

    "unknown_kernel".to_string()
}

fn find_kernel_blocks(src: &str) -> io::Result<Vec<KernelBlock>> {
    let mut blocks = Vec::new();
    let mut search_from = 0usize;

    while let Some(rel) = src[search_from..].find("kernel void") {
        let idx = search_from + rel;
        let mut start = line_start(src, idx);

        while let Some(prev_start) = prev_line_start(src, start) {
            let prev_end = line_end(src, prev_start);
            let prev = src[prev_start..prev_end].trim();
            if prev.starts_with("template") || prev.starts_with("[[") {
                start = prev_start;
                continue;
            }
            break;
        }

        let open_rel = src[idx..].find('{');
        let semi_rel = src[idx..].find(';');
        if match (open_rel, semi_rel) {
            (None, Some(_)) => true,
            (Some(open), Some(semi)) if semi < open => true,
            _ => false,
        } {
            // Declaration-only kernel entry.
            search_from = idx + "kernel void".len();
            continue;
        }
        let open_rel =
            open_rel.ok_or_else(|| io::Error::other("failed to find kernel opening brace"))?;
        let open_idx = idx + open_rel;
        let close_idx = find_matching_brace(src, open_idx)
            .ok_or_else(|| io::Error::other("failed to match kernel closing brace"))?;
        let end = close_idx + 1;

        let name = parse_export_name(&src[start..end]);
        blocks.push(KernelBlock { start, end, name });
        search_from = end;
    }

    blocks.sort_by_key(|b| b.start);
    Ok(blocks)
}

fn render_with_keep(original: &str, blocks: &[KernelBlock], keep: &[bool]) -> String {
    let mut out = String::with_capacity(original.len());
    let mut cursor = 0usize;

    for (idx, block) in blocks.iter().enumerate() {
        if block.start > cursor {
            out.push_str(&original[cursor..block.start]);
        }
        if keep[idx] {
            out.push_str(&original[block.start..block.end]);
        }
        cursor = block.end;
    }

    if cursor < original.len() {
        out.push_str(&original[cursor..]);
    }

    out
}

fn is_used_by_backend_turbo(name: &str) -> bool {
    TURBO_KERNEL_KEEP.contains(&name)
}

fn parse_host_name(line: &str) -> Option<&str> {
    let pos = line.find("host_name(\"")?;
    let start = pos + "host_name(\"".len();
    let end_rel = line[start..].find('"')?;
    Some(&line[start..start + end_rel])
}

fn strip_unused_template_host_lines(
    src: &str,
    keep_names: &HashSet<String>,
) -> (String, Vec<String>) {
    let mut out = String::with_capacity(src.len());
    let mut removed = Vec::new();
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if line.contains("template") {
            if let Some(name) = parse_host_name(line) {
                if !keep_names.contains(name) {
                    removed.push(name.to_string());
                    i += 1;
                    while i < lines.len() && !lines[i - 1].contains(';') {
                        i += 1;
                    }
                    continue;
                }
            }
        }
        out.push_str(line);
        i += 1;
    }

    (out, removed)
}

fn parse_table_name(begin_line: &str) -> Option<String> {
    let pos = begin_line.find("GGML_TABLE_BEGIN(")?;
    let mut s = &begin_line[pos + "GGML_TABLE_BEGIN(".len()..];
    let c0 = s.find(',')?;
    s = &s[c0 + 1..];
    let c1 = s.find(',')?;
    let name = s[..c1].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn find_table_blocks(src: &str) -> io::Result<Vec<TableBlock>> {
    let mut blocks = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = src[search_from..].find("GGML_TABLE_BEGIN(") {
        let idx = search_from + rel;
        let start = line_start(src, idx);
        let begin_end = line_end(src, start);
        let name = parse_table_name(&src[start..begin_end])
            .ok_or_else(|| io::Error::other("failed to parse GGML_TABLE_BEGIN name"))?;

        let end_marker_rel = src[idx..]
            .find("GGML_TABLE_END()")
            .ok_or_else(|| io::Error::other("failed to find GGML_TABLE_END for table"))?;
        let end_marker_idx = idx + end_marker_rel;
        let end_line = line_end(src, end_marker_idx);
        let end = if end_line < src.len() {
            end_line + 1
        } else {
            end_line
        };

        blocks.push(TableBlock { start, end, name });
        search_from = end;
    }
    Ok(blocks)
}

fn render_table_keep(original: &str, blocks: &[TableBlock], keep: &[bool]) -> String {
    let mut out = String::with_capacity(original.len());
    let mut cursor = 0usize;

    for (idx, block) in blocks.iter().enumerate() {
        if block.start > cursor {
            out.push_str(&original[cursor..block.start]);
        }
        if keep[idx] {
            out.push_str(&original[block.start..block.end]);
        }
        cursor = block.end;
    }

    if cursor < original.len() {
        out.push_str(&original[cursor..]);
    }

    out
}

fn contains_ident(src: &str, ident: &str) -> bool {
    let mut offset = 0usize;
    while let Some(rel) = src[offset..].find(ident) {
        let idx = offset + rel;
        let before_ok = idx == 0
            || !src.as_bytes()[idx - 1].is_ascii_alphanumeric() && src.as_bytes()[idx - 1] != b'_';
        let after_idx = idx + ident.len();
        let after_ok = after_idx >= src.len()
            || !src.as_bytes()[after_idx].is_ascii_alphanumeric()
                && src.as_bytes()[after_idx] != b'_';
        if before_ok && after_ok {
            return true;
        }
        offset = idx + ident.len();
    }
    false
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new(METAL_PATH).exists() {
        return Err(format!("metal source not found at {}", METAL_PATH).into());
    }

    if !Path::new(METAL_BACKUP_PATH).exists() {
        fs::copy(METAL_PATH, METAL_BACKUP_PATH)?;
    }

    let original = fs::read_to_string(METAL_PATH)?;
    let before_bytes = original.len();
    let blocks = find_kernel_blocks(&original)?;
    if blocks.is_empty() {
        return Err("no kernel blocks found in metal source".into());
    }

    let mut keep: Vec<bool> = blocks
        .iter()
        .map(|b| is_used_by_backend_turbo(&b.name))
        .collect();

    // Keep dependency closure: if a removed kernel symbol is still referenced by
    // remaining source text, keep that block to preserve compile/link integrity.
    loop {
        let candidate = render_with_keep(&original, &blocks, &keep);
        let mut changed = false;
        for (i, block) in blocks.iter().enumerate() {
            if keep[i] {
                continue;
            }
            let has_ref = candidate.contains(&format!("{}(", block.name))
                || candidate.contains(&format!("{}<", block.name));
            if !block.name.is_empty() && has_ref {
                keep[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let stripped_blocks = render_with_keep(&original, &blocks, &keep);
    let mut keep_names: HashSet<String> =
        TURBO_KERNEL_KEEP.iter().map(|name| (*name).to_string()).collect();
    for (block, keep_block) in blocks.iter().zip(keep.iter()) {
        if *keep_block {
            keep_names.insert(block.name.clone());
        }
    }
    let (stripped, removed_template_hosts) =
        strip_unused_template_host_lines(&stripped_blocks, &keep_names);
    let after_bytes = stripped.len();
    fs::write(METAL_PATH, &stripped)?;

    let removed_kernel_blocks: Vec<String> = blocks
        .iter()
        .zip(keep.iter())
        .filter_map(|(b, k)| if *k { None } else { Some(b.name.clone()) })
        .collect();
    let mut removed: Vec<String> = removed_kernel_blocks.clone();
    for name in removed_template_hosts {
        removed.push(format!("template:{}", name));
    }
    removed.sort();
    removed.dedup();
    // Strip unreferenced quantization tables from ggml-common.h based on the
    // final stripped metal source.
    let common_original = fs::read_to_string(COMMON_H_PATH)?;
    let common_before_bytes = common_original.len();
    let table_blocks = find_table_blocks(&common_original)?;
    let impl_h_source = fs::read_to_string(METAL_IMPL_H_PATH)?;
    let table_usage_source = format!("{}\n{}", stripped, impl_h_source);
    let table_keep: Vec<bool> = table_blocks
        .iter()
        .map(|tb| {
            let used_by_metal = contains_ident(&table_usage_source, &tb.name);
            let used_before = contains_ident(&common_original[..tb.start], &tb.name);
            let used_after = contains_ident(&common_original[tb.end..], &tb.name);
            used_by_metal || used_before || used_after
        })
        .collect();
    let common_stripped = render_table_keep(&common_original, &table_blocks, &table_keep);
    let common_after_bytes = common_stripped.len();
    fs::write(COMMON_H_PATH, common_stripped)?;

    let removed_tables: Vec<String> = table_blocks
        .iter()
        .zip(table_keep.iter())
        .filter_map(|(tb, keep_tb)| {
            if *keep_tb {
                None
            } else {
                Some(format!("table:{}", tb.name))
            }
        })
        .collect();
    removed.extend(removed_tables.clone());
    removed.sort();
    removed.dedup();
    fs::write(REMOVED_REPORT_PATH, removed.join("\n"))?;

    eprintln!(
        "[metal-strip] kernels total={} kept={} removed={}",
        blocks.len(),
        blocks.len().saturating_sub(removed_kernel_blocks.len()),
        removed_kernel_blocks.len()
    );
    eprintln!(
        "[metal-strip] templates removed={}",
        removed.len().saturating_sub(removed_kernel_blocks.len())
    );
    eprintln!(
        "[metal-strip] bytes before={} after={} delta={} ({:.1}%)",
        before_bytes,
        after_bytes,
        (before_bytes as isize - after_bytes as isize),
        100.0 * (before_bytes.saturating_sub(after_bytes) as f64) / before_bytes.max(1) as f64
    );
    eprintln!(
        "[metal-strip] ggml-common tables total={} kept={} removed={}",
        table_blocks.len(),
        table_keep.iter().filter(|k| **k).count(),
        removed_tables.len()
    );
    eprintln!(
        "[metal-strip] ggml-common bytes before={} after={} delta={} ({:.1}%)",
        common_before_bytes,
        common_after_bytes,
        (common_before_bytes as isize - common_after_bytes as isize),
        100.0 * (common_before_bytes.saturating_sub(common_after_bytes) as f64)
            / common_before_bytes.max(1) as f64
    );
    eprintln!("[metal-strip] removed list: {}", REMOVED_REPORT_PATH);

    Ok(())
}
