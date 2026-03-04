#!/usr/bin/env python3
# Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the
# Alliance for Open Media Patent License 1.0 was not distributed with this
# source code in the PATENTS file, you can obtain it at
# https://www.aomedia.org/license/patent-license.

import os
import platform
from typing import Any, Dict

import yaml

CONFIG_NAME = "configs/config.yaml"


def update_values(values, placeholders):
    class Default(dict):
        def __missing__(self, key):
            return f"{{{key}}}"

    if isinstance(values, dict):
        for key, value in values.items():
            values[key] = update_values(value, placeholders)
    elif isinstance(values, list):
        values = [update_values(value, placeholders) for value in values]
    elif isinstance(values, str):
        values = os.path.expanduser(values.format_map(Default(placeholders)))
    return values


class ConfigManager:
    _instance = None

    def __new__(cls, config_path=None):
        if cls._instance is None:
            cls._instance = super(ConfigManager, cls).__new__(cls)
            cls._instance._initialized = False
        return cls._instance

    def __init__(self, config_path=None):
        if self._initialized:
            return

        if config_path is None:
            config_path = CONFIG_NAME

        with open(config_path, "r") as file:
            self.config = yaml.safe_load(file)
        self.validate_config()
        self._initialized = True

    def validate_config(self) -> None:
        required_sections = [
            "paths",
            "codecs",
            "metrics",
            "binaries",
            "common_settings",
            "encoder_settings",
            "decoder_settings",
        ]
        for section in required_sections:
            if section not in self.config:
                raise ValueError(f"Missing required config section: {section}")

        if self.config.get("placeholders", {}):
            update_values(self.config, self.config["placeholders"])

        allowed_codecs = self.config["codecs"].get("allowed_codecs", [])
        if not allowed_codecs:
            raise ValueError("Missing required section: codecs.allowed_codecs")

        # check input/output extensions
        for codec_type, codec_type_dict in self.config["common_settings"].items():
            dec_ext = codec_type_dict["decode_extension"]
            for codec in allowed_codecs:
                if codec in codec_type_dict:
                    src_ext = codec_type_dict[codec]["input_extension"]
                    if (src_ext == "yuv") != (dec_ext == "yuv"):
                        raise ValueError(
                            f"Input/decode extensions should either both be yuv or neither: {codec_type}/{codec} input {src_ext}, decode {dec_ext}"
                        )

        paths = self.config["paths"]
        paths["source_data_dir"] = self.config["dataset"]["source_dir"]
        dataset_name = os.path.basename(paths["source_data_dir"])
        tmp_dir = self.config["dataset"]["tmp_dir"]
        paths["source_png_dir"] = os.path.join(tmp_dir, f"{dataset_name}_png")
        paths["source_y4m_dir"] = os.path.join(tmp_dir, f"{dataset_name}_y4m")
        paths["source_yuv_dir"] = os.path.join(tmp_dir, f"{dataset_name}_yuv")

    def get_paths(self) -> Dict[str, Any]:
        return self.config["paths"]

    def get_binaries(self) -> Dict[str, str]:
        bins = self.config["binaries"]
        if platform.system() == "Darwin":
            return bins.get("mac_arm64", bins)
        return bins.get("linux_x86_64", bins)

    def get_common_settings(self) -> Dict[str, Dict[str, Any]]:
        return self.config["common_settings"]

    def get_encoder_settings(self) -> Dict[str, Dict[str, Any]]:
        return self.config["encoder_settings"]

    def get_decoder_settings(self) -> Dict[str, Dict[str, Any]]:
        return self.config["decoder_settings"]

    def get_settings(self) -> Dict[str, Any]:
        return self.config["settings"]

    def get_codecs(self) -> Dict[str, Any]:
        return self.config["codecs"]

    def get_metrics(self) -> Dict[str, Any]:
        return self.config["metrics"]
