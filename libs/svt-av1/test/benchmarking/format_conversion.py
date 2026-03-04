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
import re
import subprocess
import sys
from logging import Logger
from pathlib import Path
from typing import Any, Dict

import utils
from config_manager import ConfigManager

config_path = sys.argv[1] if len(sys.argv) > 1 else None
config_manager = ConfigManager(config_path=config_path)

BINARIES: Dict[str, str] = config_manager.get_binaries()
COMMON_SETTINGS: Dict[str, Dict[str, Any]] = config_manager.get_common_settings()

FFMPEG_BIN: str = BINARIES["ffmpeg"]
FFPROBE_BIN: str = BINARIES["ffprobe"]

COLOR_PARAMS: str = (
    "in_color_matrix=bt601:in_range=pc:out_color_matrix=bt601:out_range=pc"
)
CHROMA_PARAMS: str = (
    "flags=lanczos+accurate_rnd+full_chroma_int:sws_dither=none:param0=5"
)
SCALING_PARAMS: str = f"scale={COLOR_PARAMS}:{CHROMA_PARAMS},"
# SCALING_PARAMS: str = ""


# convert to y4m limited color range, should be VMAF compliant
def convert_to_y4m(file: str, target_dir: str, logger: Logger):
    output_file = os.path.join(target_dir, Path(file).stem + ".y4m")

    if os.path.exists(output_file):
        return output_file

    if file.endswith(".y4m"):
        try:
            os.symlink(file, output_file)
            logger.info(f"Linked Y4M file: {file} -> {output_file}")
        except Exception as e:
            logger.error(f"Error linking Y4M file {file} - {e}")
    else:
        cmd = [
            FFMPEG_BIN,
            "-loglevel",
            "error",
            "-i",
            file,
            "-vf",
            SCALING_PARAMS + "format=yuv420p",
            "-pix_fmt",
            "yuv420p",
            "-color_range",
            "pc",
            "-y",
            output_file,
        ]
        if file.endswith(".yuv"):
            # insert resolution before "-i"
            width, height, _ = utils.get_file_desc(Path(file).stem)
            cmd[3:3] = ("-s:v", f"{width}x{height}")

        subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return output_file


def convert_to_png(file: str, target_dir: str, logger: Logger):
    output_file = os.path.join(target_dir, Path(file).stem + ".png")

    if os.path.exists(output_file):
        return output_file

    if file.endswith(".png"):
        try:
            os.symlink(file, output_file)
            logger.info(f"Linked PNG file: {file} -> {output_file}")
        except Exception as e:
            logger.error(f"Error linking PNG file {file} - {e}")
    else:
        cmd = [
            FFMPEG_BIN,
            "-loglevel",
            "error",
            "-i",
            file,
            "-vf",
            SCALING_PARAMS + "format=rgb24",
            "-f",
            "image2",
            "-update",
            "1",
            "-frames:v",
            "1",
            "-y",
            "-compression_level",
            "0",
            output_file,
        ]
        if file.endswith(".yuv"):
            # insert resolution before "-i"
            width, height, _ = utils.get_file_desc(Path(file).stem)
            cmd[3:3] = ("-s:v", f"{width}x{height}")

        subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return output_file


def get_resolution(filename):
    """
    Given a file in the png or y4m format, will return a tuple for (width, height)
    """
    cmd = [
        "ffprobe",
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height,r_frame_rate",
        "-of",
        "csv=p=0:s=x",
        filename,
    ]
    result = subprocess.run(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    pattern = re.compile(
        r"(?P<width>\d+)x(?P<height>\d+)x(?P<fps_n>\d+)/(?P<fps_d>\d+)"
    )
    match = pattern.search(result.stdout.strip())
    if match:
        width = int(match.group("width"))
        height = int(match.group("height"))
        fps_n = int(match.group("fps_n"))
        fps_d = int(match.group("fps_d"))
        return width, height, fps_n // fps_d
    raise ValueError(f"Could not get resolution for {filename}")


def convert_to_yuv(file: str, target_dir: str, logger: Logger):
    output_file = os.path.join(target_dir, Path(file).stem)

    if file.endswith(".yuv"):
        # If input is already YUV, just copy it
        target_path = output_file + ".yuv"
        try:
            if not os.path.exists(target_path):
                os.symlink(file, target_path)
                logger.info(f"Linked YUV file: {file} -> {target_path}")
        except Exception as e:
            logger.error(f"Error linking YUV file {file} - {e}")
        return target_path

    if file.endswith(".y4m") or file.endswith(".png"):
        width, height, fps = get_resolution(file)
        target_path = output_file + f"_{width}x{height}_{fps}.yuv"
        if not os.path.exists(target_path):
            # Convert Y4M/PNG to YUV
            cmd = [
                FFMPEG_BIN,
                "-loglevel",
                "error",
                "-i",
                file,
                "-vf",
                SCALING_PARAMS + "format=yuv420p",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "pc",
                "-y",
                target_path,
            ]
            subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        return target_path

    logger.error(f"Unexpected file type - {file}")
    return None


def detect_source_format(source_data_dir: str) -> str:
    """Detect the file format of the source data directory"""
    file_formats = ["y4m", "yuv", "png", "jpg", "jpeg"]
    extensions_on_disk = [f.split(".")[-1] for f in os.listdir(source_data_dir)]
    for file_format in file_formats:
        if file_format in extensions_on_disk:
            return file_format
    raise ValueError(f"Could not detect file format of {source_data_dir}")


def generate_input_data_reformats(input_ext, source_data_dir, target_data_dirs, logger):
    filenames = [
        os.path.join(source_data_dir, f)
        for f in os.listdir(source_data_dir)
        if f.endswith(input_ext)
    ]

    formats = set()
    for _, codec_settings in COMMON_SETTINGS.items():
        for codec_name in config_manager.get_codecs()["allowed_codecs"]:
            if codec_name in codec_settings:
                formats.add(codec_settings[codec_name]["input_extension"])

    allow_metrics = {"vmaf": False, "ssimulacra2": False, "ms_ssim": False}
    for m in config_manager.get_metrics().get("allowed_metrics", []):
        allow_metrics[m] = True

    if allow_metrics["ssimulacra2"]:
        formats.add("png")
    if (allow_metrics["vmaf"] or allow_metrics["ms_ssim"]) and not ("yuv" in formats):
        formats.add("y4m")

    _ = [os.makedirs(x, exist_ok=True) for x in target_data_dirs.values()]
    converters = {
        "y4m": convert_to_y4m,
        "yuv": convert_to_yuv,
        "png": convert_to_png,
    }
    for filename in filenames:
        for fmt in formats:
            target_data_dir = target_data_dirs[fmt]
            converters[fmt](filename, target_data_dir, logger)
