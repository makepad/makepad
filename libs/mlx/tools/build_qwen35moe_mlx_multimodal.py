#!/usr/bin/env python3
import argparse
import json
import shutil
import struct
from pathlib import Path


def load_json(path: Path):
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def save_json(path: Path, value):
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, ensure_ascii=False)
        handle.write("\n")


def load_safetensors_header(path: Path):
    with open(path, "rb") as handle:
        header_len = struct.unpack("<Q", handle.read(8))[0]
        header = json.loads(handle.read(header_len).decode("utf-8"))
    return header_len, header


def tensor_payload_size(entry: dict) -> int:
    start, end = entry["data_offsets"]
    return end - start


def tensor_element_count(entry: dict) -> int:
    count = 1
    for dim in entry["shape"]:
        count *= dim
    return count


def build_vision_shard(
    original_model_dir: Path,
    original_index: dict,
    output_path: Path,
):
    weight_map = original_index["weight_map"]
    visual_keys = sorted(key for key in weight_map if key.startswith("model.visual"))
    if not visual_keys:
        raise ValueError("original model has no model.visual tensors")

    source_headers = {}
    output_header = {}
    output_payload = bytearray()
    parameter_count = 0

    for key in visual_keys:
        shard_name = weight_map[key]
        shard_path = original_model_dir / shard_name
        if shard_path not in source_headers:
            source_headers[shard_path] = load_safetensors_header(shard_path)
        _, source_header = source_headers[shard_path]
        entry = source_header[key]
        data_start, data_end = entry["data_offsets"]
        renamed_key = key.replace("model.visual", "vision_tower", 1)
        parameter_count += tensor_element_count(entry)

        with open(shard_path, "rb") as handle:
            payload_base = 8 + source_headers[shard_path][0]
            handle.seek(payload_base + data_start)
            tensor_bytes = handle.read(data_end - data_start)

        new_start = len(output_payload)
        output_payload.extend(tensor_bytes)
        new_end = len(output_payload)
        output_header[renamed_key] = {
            "dtype": entry["dtype"],
            "shape": entry["shape"],
            "data_offsets": [new_start, new_end],
        }

    header_bytes = json.dumps(output_header, separators=(",", ":"), ensure_ascii=False).encode(
        "utf-8"
    )
    with open(output_path, "wb") as handle:
        handle.write(struct.pack("<Q", len(header_bytes)))
        handle.write(header_bytes)
        handle.write(output_payload)

    return {
        "tensor_count": len(output_header),
        "payload_size": len(output_payload),
        "parameter_count": parameter_count,
        "weight_map": {name: output_path.name for name in output_header},
    }


def copy_support_files(original_model_dir: Path, output_dir: Path):
    output_dir.mkdir(parents=True, exist_ok=True)
    for item in original_model_dir.iterdir():
        if item.name == "model.safetensors.index.json":
            continue
        if item.name.startswith("model-") and item.suffix == ".safetensors":
            continue
        destination = output_dir / item.name
        if item.is_dir():
            if destination.exists():
                shutil.rmtree(destination)
            shutil.copytree(item, destination)
        else:
            shutil.copy2(item, destination)


def copy_quantized_text_shards(quantized_model_dir: Path, output_dir: Path):
    index = load_json(quantized_model_dir / "model.safetensors.index.json")
    for shard_name in sorted(set(index["weight_map"].values())):
        shutil.copy2(quantized_model_dir / shard_name, output_dir / shard_name)
    return index


def merged_config(original_config: dict, quantized_config: dict):
    config = dict(original_config)
    config["text_config"] = quantized_config["text_config"]
    config["quantization"] = quantized_config["quantization"]
    config["quantization_config"] = quantized_config.get(
        "quantization_config", quantized_config["quantization"]
    )
    if "dtype" in quantized_config:
        config["dtype"] = quantized_config["dtype"]
    return config


def build_merged_index(
    quantized_index: dict,
    vision_weight_map: dict,
    vision_payload_size: int,
    vision_parameter_count: int,
):
    merged_weight_map = dict(quantized_index["weight_map"])
    merged_weight_map.update(vision_weight_map)

    metadata = dict(quantized_index.get("metadata", {}))
    total_size = metadata.get("total_size")
    if isinstance(total_size, str):
        try:
            total_size = int(float(total_size))
        except ValueError:
            total_size = None
    if not isinstance(total_size, int):
        total_size = 0
    metadata["total_size"] = total_size + vision_payload_size

    total_parameters = metadata.get("total_parameters")
    if isinstance(total_parameters, str):
        try:
            total_parameters = int(float(total_parameters))
        except ValueError:
            total_parameters = None
    if isinstance(total_parameters, int):
        metadata["total_parameters"] = total_parameters + vision_parameter_count

    return {
        "metadata": metadata,
        "weight_map": merged_weight_map,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Build an MLX multimodal Qwen3.5-MoE model by combining a text-quantized MLX export with the original vision tower."
    )
    parser.add_argument("original_model_dir", type=Path)
    parser.add_argument("quantized_text_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument(
        "--vision-shard-name",
        default="vision-00001-of-00001.safetensors",
        help="Filename for the rewritten vision safetensors shard.",
    )
    args = parser.parse_args()

    original_model_dir = args.original_model_dir.resolve()
    quantized_text_dir = args.quantized_text_dir.resolve()
    output_dir = args.output_dir.resolve()

    copy_support_files(original_model_dir, output_dir)
    quantized_index = copy_quantized_text_shards(quantized_text_dir, output_dir)

    original_index = load_json(original_model_dir / "model.safetensors.index.json")
    vision_shard = build_vision_shard(
        original_model_dir,
        original_index,
        output_dir / args.vision_shard_name,
    )

    quantized_config = load_json(quantized_text_dir / "config.json")
    original_config = load_json(original_model_dir / "config.json")
    save_json(output_dir / "config.json", merged_config(original_config, quantized_config))
    save_json(
        output_dir / "model.safetensors.index.json",
        build_merged_index(
            quantized_index,
            vision_shard["weight_map"],
            vision_shard["payload_size"],
            vision_shard["parameter_count"],
        ),
    )

    print(f"wrote {output_dir}")
    print(f"quantized_text_tensors={len(quantized_index['weight_map'])}")
    print(f"vision_tensors={vision_shard['tensor_count']}")
    print(f"vision_payload_bytes={vision_shard['payload_size']}")
    print(
        f"merged_tensors={len(quantized_index['weight_map']) + vision_shard['tensor_count']}"
    )


if __name__ == "__main__":
    main()
