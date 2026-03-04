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

"""
Run using one of the following options:

  # Use default --read_qm mode; will read the xml and ssimulacra files for quality metrics
  python summary.py config.yaml

  # Read the csv files provided for encoding, decoding, and quality metrics
  python summary.py config.yaml --use_enc_qm_logs

  # Process per-image logs where each per image contains all necessary columns;
  # Used for merging data from different datasets
  python summary.py config.yaml --use_per_image_log file1.csv file2.csv
"""

import argparse
import os
from typing import List

import pandas as pd

from analysis_and_plotting import create_per_image_rd_plots, run_bd_rate_analysis
from config_manager import ConfigManager
from data_readers import DataReaders

# Configurations will be loaded after parsing arguments
config_manager = None
PATHS = None
COMMON_SETTINGS = None
OUTPUT_DIR = None
PER_IMAGE_FILE = None


def setup_argument_parser():
    """Set up and parse command line arguments."""
    parser = argparse.ArgumentParser(
        description="Image Coding Summary Tool",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python summary.py config.yaml                          # Use default --read_qm mode
  python summary.py config.yaml --use_enc_qm_logs        # Read encoding CSV and QM CSV files
  python summary.py config.yaml --use_per_image_log file1.csv file2.csv  # Process per-image logs
        """,
    )

    # Required config file argument
    parser.add_argument(
        "config_file",
        help="Path to the configuration YAML file that defines paths, encoders, and settings",
    )

    # Mode selection group - make it mutually exclusive but not required (--read_qm is default)
    group = parser.add_mutually_exclusive_group(required=False)
    group.add_argument(
        "--use_enc_qm_logs",
        action="store_true",
        help="Read encoding CSV and QM CSV files, merge them and run plotting/BD-rate analysis",
    )
    group.add_argument(
        "--use_per_image_log",
        nargs="+",
        help="Accept list of CSV files with per_image results and combine them",
    )
    group.add_argument(
        "--read_qm",
        action="store_true",
        default=True,
        help="Read quality metrics from qm_dir and create per_image dataframe (default mode)",
    )

    return parser.parse_args()


def process_enc_qm_logs(data_readers: DataReaders) -> pd.DataFrame:
    """
    Process encoding CSV and QM CSV files, merge them and create per_image dataframe.

    Args:
        data_readers: Centralized data reader instance

    Returns:
        DataFrame with per-image results
    """
    print("Using encoding CSV and QM CSV logs mode...")

    # Parse per-image data and save CSV
    encoding_df = data_readers.read_encoding_csv_data()
    decode_qm_df = data_readers.read_decode_qm_csv_data()
    assert encoding_df is not None and decode_qm_df is not None, "cannot read CSV files"
    per_image_data = data_readers.merge_enc_dec_qm_dfs(encoding_df, decode_qm_df)

    return per_image_data


def process_per_image_logs(filenames: List[str]) -> pd.DataFrame:
    """
    Process multiple per-image CSV files and combine them.

    Args:
        filenames: List of CSV file paths to process

    Returns:
        DataFrame with combined per-image results
    """
    print(f"Using per-image log files mode with {len(filenames)} files...")

    filenames = [fname.strip() for fname in filenames]
    dfs = [DataReaders.read_preprocessed_per_image_data(fname) for fname in filenames]
    per_image_data = pd.concat(dfs, axis=0)

    # if there are rows with no dataset specified that will be a problem, remove dataset from columns.
    if (
        "dataset" in per_image_data.columns
        and per_image_data["dataset"].isna().sum() > 0
    ):
        per_image_data = per_image_data.drop(columns=["dataset"])

    return per_image_data


def process_qm_directory(data_readers: DataReaders) -> pd.DataFrame:
    """
    Read quality metrics from qm_dir, if available, join them with encoding csv data, to create per_image dataframe.

    Args:
        data_readers: Centralized data reader instance

    Returns:
        DataFrame with per-image results including quality metrics, file sizes, and runtime
    """
    print("Reading quality metrics from qm_dir...")

    # Get qm_dir from config
    assert PATHS
    qm_dir = PATHS.get("qm_dir", "")

    # Read quality metrics from qm_dir using centralized reader
    qm_df = data_readers.read_qm_logs_from_dir(qm_dir)

    if qm_df.empty:
        return qm_df

    # Try to get encoding CSV data using centralized reader
    encoding_data = data_readers.read_encoding_csv_data()

    # Merge with encoding data if available
    if encoding_data is None or encoding_data.empty:
        # No encoding CSV available, fall back to file size calculation from disk
        print("No encoding CSV available, calculating file sizes from disk...")
        qm_df = data_readers.add_file_size_from_disk(qm_df)
        return qm_df

    print("Found encoding CSV data, merging with QM data...")

    # Prepare merge keys for both dataframes
    qm_df_for_merge = qm_df.copy()
    encoding_df_for_merge = encoding_data.copy()

    # Column renaming is now handled in _safe_read_csv, no need for additional renaming
    # encoding_df_for_merge already has standardized column names

    # Merge on image, encoder, speed, quality, threads
    merge_keys = ["image", "encoder", "speed", "quality", "threads"]

    # Check if all merge keys exist in both dataframes
    missing_keys_qm = [key for key in merge_keys if key not in qm_df_for_merge.columns]
    missing_keys_enc = [
        key for key in merge_keys if key not in encoding_df_for_merge.columns
    ]

    if missing_keys_qm or missing_keys_enc:
        print(
            f"Warning: Cannot merge - missing keys. QM missing: {missing_keys_qm}, Encoding missing: {missing_keys_enc}"
        )
        return pd.DataFrame()

    # Perform the merge
    merge_columns = merge_keys + [
        i for i in ["encode_time", "file_size_mb"] if i in encoding_df_for_merge.columns
    ]
    per_image_data = pd.merge(
        qm_df_for_merge,
        encoding_df_for_merge[merge_columns],
        on=merge_keys,
        how="left",
    )

    print(f"Successfully merged {len(per_image_data)} records with encoding data")

    return per_image_data


def run_analysis(per_image_data: pd.DataFrame, per_image_csv_path: str) -> None:
    """
    Run BD-rate analysis and create RD plots using per-image data.

    Args:
        per_image_data: DataFrame with per-image results
        per_image_csv_path: Path where the per-image CSV should be saved
    """

    assert config_manager
    allowed_codecs = config_manager.get_codecs().get("allowed_codecs", [])
    per_image_data = per_image_data[per_image_data["encoder"].isin(allowed_codecs)]

    if per_image_data.empty:
        print("No per-image data available for analysis")
        return

    # Save per-image CSV
    per_image_data.to_csv(per_image_csv_path, index=False)
    print(f"Per-image results saved to: {per_image_csv_path}")

    assert OUTPUT_DIR

    allowed_metrics = config_manager.get_metrics().get("allowed_metrics", [])
    quality_metrics = []
    stream_metrics = []
    if "ssimulacra2" in allowed_metrics:
        quality_metrics.append("ssimulacra2")
    if "psnr" in allowed_metrics:
        quality_metrics.append("psnr_y")
        quality_metrics.append("psnr_cb")
        quality_metrics.append("psnr_cr")
    if "vmaf" in allowed_metrics:
        quality_metrics.append("vmaf")
        quality_metrics.append("vmaf_neg")
    if "ssim" in allowed_metrics:
        quality_metrics.append("ssim")
    if "ms_ssim" in allowed_metrics:
        quality_metrics.append("ms_ssim")
    if "vbv" in allowed_metrics:
        stream_metrics.append("vbv_delay_p95")
        stream_metrics.append("vbv_delay_p50")

    # Create per-image RD plots using new analysis module
    create_per_image_rd_plots(
        per_image_data, quality_metrics + stream_metrics, OUTPUT_DIR
    )

    # Run BD-rate analysis using new analysis module if enabled
    run_bd_rate_analysis(
        per_image_csv_path, quality_metrics, OUTPUT_DIR, config_manager.get_metrics()
    )


def initialize_configuration(config_file: str) -> None:
    """Initialize global configuration variables from config file."""
    global config_manager
    global PATHS, COMMON_SETTINGS, OUTPUT_DIR, PER_IMAGE_FILE

    config_manager = ConfigManager(config_path=config_file)
    PATHS = config_manager.get_paths()
    COMMON_SETTINGS = config_manager.get_common_settings()

    # Get output directory from config, default to current directory
    OUTPUT_DIR = PATHS.get("metrics_dir", ".")
    PER_IMAGE_FILE = os.path.join(OUTPUT_DIR, "unified_per_image_results.csv")


def main() -> None:
    """Main function to orchestrate the image coding summary analysis."""
    # Parse command line arguments first
    args = setup_argument_parser()

    # Initialize configuration from the provided config file
    initialize_configuration(args.config_file)

    assert OUTPUT_DIR
    assert PATHS
    assert COMMON_SETTINGS
    assert PER_IMAGE_FILE

    # Create output directory
    os.makedirs(OUTPUT_DIR, exist_ok=True)

    # Initialize centralized data readers
    data_readers = DataReaders(PATHS, COMMON_SETTINGS)

    # Process data based on the selected branch (default is --read_qm)
    per_image_data = pd.DataFrame()

    if args.use_enc_qm_logs:
        per_image_data = process_enc_qm_logs(data_readers)

    elif args.use_per_image_log:
        per_image_data = process_per_image_logs(args.use_per_image_log)

    else:  # Default to --read_qm mode
        per_image_data = process_qm_directory(data_readers)

    # Run analysis on the per-image data
    run_analysis(per_image_data, PER_IMAGE_FILE)


if __name__ == "__main__":
    main()
