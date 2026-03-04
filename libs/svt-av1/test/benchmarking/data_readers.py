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
import xml.etree.ElementTree as ET
from typing import Any, Dict, List, Optional, Tuple

import pandas as pd

# Constants
BYTES_IN_MB = 1024.0 * 1024.0  # Conversion factor from bytes to megabytes


class DataReaders:
    """Centralized data readers for encoding, decoding, and QM logs."""

    def __init__(
        self,
        paths: Dict[str, str],
        common_settings: Dict,
    ):
        self.paths = paths
        self.common_settings = common_settings

        # File paths
        self.encoding_csv_file = paths["enc_csv_path"]
        self.decoding_csv_file = paths["dec_csv_path"]
        self.encoded_dir = paths["encoded_dir"]

    @staticmethod
    def _get_file_size_mb(file_path: str) -> float:
        """Get file size in MB for a single file."""
        try:
            if os.path.exists(file_path):
                size_bytes = os.path.getsize(file_path)
                return round(size_bytes / BYTES_IN_MB, 4)
        except OSError:
            pass
        return float("nan")

    @staticmethod
    def _safe_read_csv(file_path: str, **kwargs) -> Optional[pd.DataFrame]:
        """
        Safely read CSV file with error handling and standardized column renaming.

        Args:
            file_path: Path to CSV file
            **kwargs: Additional arguments to pass to pd.read_csv

        Returns:
            DataFrame with standardized column names or None if reading failed
        """
        try:
            if not os.path.exists(file_path):
                return None

            df = pd.read_csv(file_path, **kwargs)

            # Comprehensive column renaming to standardize across all CSV files
            column_renames = {
                # Encoder naming standardization
                "encoder_name": "encoder",
                # File naming standardization
                "input_file": "image_file",
            }

            # Apply renaming only for columns that exist
            actual_renames = {
                k: v for k, v in column_renames.items() if k in df.columns
            }
            if actual_renames:
                df = df.rename(columns=actual_renames)

            return df

        except Exception as e:
            print(f"Warning: Could not read CSV file {file_path}: {e}")
            return None

    def _parse_encoder_from_path_part(
        self, encoder_part: str, codec_settings: Dict
    ) -> Tuple[Optional[str], int]:
        """
        Parse encoder and speed from encoder path part using codec settings.

        Args:
            encoder_part: First part of path (e.g., "svtav1_enc_speed7")
            codec_settings: Configuration settings for a specific codec type

        Returns:
            Tuple of (encoder, speed) or (None, 0) if parsing fails
        """
        dir_format = codec_settings.get("dir_format", {})
        speed_suffix = dir_format.get("speed_suffix", "")

        for encoder_name in codec_settings.keys():
            if encoder_name in [
                "encode_extension",
                "decode_extension",
                "dir_format",
            ]:
                continue

            if speed_suffix:
                # Check if encoder_part matches encoder_name + speed pattern
                if encoder_part.startswith(encoder_name):
                    remaining_part = encoder_part[len(encoder_name) :]
                    # Extract speed from the suffix pattern
                    if "_speed" in speed_suffix:
                        if remaining_part.startswith("_speed"):
                            try:
                                speed = int(remaining_part[6:])  # Remove "_speed"
                                return encoder_name, speed
                            except ValueError:
                                continue
                    else:
                        # Custom speed suffix pattern - handle empty case
                        if remaining_part == "":
                            return encoder_name, 0
            else:
                # No speed suffix, direct match
                if encoder_part == encoder_name:
                    return encoder_name, 0

        return None, 0

    def _parse_quality_from_path_part(
        self, quality_part: str, quality_param: str
    ) -> Tuple[Optional[int], Optional[int]]:
        """
        Parse quality and threads from quality path part.

        Args:
            quality_part: Second part of path (e.g., "qp31_t1")
            quality_param: Quality parameter name from config (e.g., "qp", "q")

        Returns:
            Tuple of (quality, threads) or (None, None) if parsing fails
        """
        if quality_part.startswith(quality_param) and "_t" in quality_part:
            param_part, threads_part = quality_part.split("_t", 1)
            try:
                quality = int(
                    param_part[len(quality_param) :]
                )  # Remove quality_param prefix
                threads = int(threads_part)
                return quality, threads
            except ValueError:
                return None, None
        return None, None

    def _parse_directory_path(
        self, rel_path: str
    ) -> Tuple[Optional[str], int, Optional[int], Optional[int]]:
        """
        Parse directory path to extract encoder, speed, quality, and threads using config.

        Args:
            rel_path: Relative path from qm_dir (e.g., "svtav1_enc_speed7/qp31_t1")

        Returns:
            Tuple of (encoder, speed, quality, threads) or (None, 0, None, None) if parsing fails
        """
        path_parts = rel_path.split(os.sep)
        if len(path_parts) < 2:
            return None, 0, None, None

        encoder_part = path_parts[0]
        quality_part = path_parts[1]

        # Try to match encoder and speed based on config
        for _, codec_settings in self.common_settings.items():
            encoder, speed = self._parse_encoder_from_path_part(
                encoder_part, codec_settings
            )

            if encoder:
                # Get quality parameter for this codec type
                dir_format = codec_settings.get("dir_format", {})
                quality_param = dir_format.get("quality_param", "q")

                # Parse quality and threads from second part
                quality, threads = self._parse_quality_from_path_part(
                    quality_part, quality_param
                )

                return encoder, speed, quality, threads

        return None, 0, None, None

    def _collect_files_by_directory(self, qm_dir: str) -> Dict[str, Dict[str, Any]]:
        """
        Walk through qm_dir and collect XML and ssimulacra files by directory.

        Args:
            qm_dir: Directory containing quality metric logs

        Returns:
            Dictionary mapping directory paths to {"xml": [...], "ssimulacra": [...]} file lists
        """
        files_by_dir = {}

        for root, _, files in os.walk(qm_dir):
            xml_files = [f for f in files if f.endswith(".xml")]
            ssimulacra_files = [f for f in files if f.endswith(".ssimulcra")]

            if xml_files or ssimulacra_files:
                rel_path = os.path.relpath(root, qm_dir)
                files_by_dir[root] = {
                    "xml": xml_files,
                    "ssimulacra": ssimulacra_files,
                    "rel_path": rel_path,
                }

        return files_by_dir

    def _read_xml_file(self, xml_path: str) -> Optional[Dict[str, float]]:
        """
        Read VMAF metrics from an XML file.

        Args:
            xml_path: Path to XML file

        Returns:
            Dictionary of metric name -> value, or None if reading failed
        """
        try:
            tree = ET.parse(xml_path)
            vmaf_data = tree.getroot()

            metrics = {}
            # Extract metrics from pooled_metrics section
            for metric_elem in vmaf_data.findall("./pooled_metrics/metric"):
                name = metric_elem.get("name", "")
                if name.startswith("float_"):
                    name = name[6:]
                elif name.startswith("integer_"):
                    name = name[8:]
                mean = float(metric_elem.get("mean", ""))
                metrics[name] = mean

            return metrics

        except Exception as e:
            print(f"Warning: Could not parse XML file {xml_path}: {e}")
            return None

    def _read_ssimulacra_file(self, ssimulacra_path: str) -> Optional[float]:
        """
        Read SSIMULACRA2 value from a ssimulacra file.

        Args:
            ssimulacra_path: Path to ssimulacra file

        Returns:
            SSIMULACRA2 value or None if reading failed
        """
        try:
            with open(ssimulacra_path, "r") as f:
                return float(f.read().strip())
        except (ValueError, FileNotFoundError) as e:
            print(f"Warning: Could not read ssimulacra file {ssimulacra_path}: {e}")
            return None

    def _process_files_in_directory(
        self,
        dir_path: str,
        files_info: Dict[str, List[str]],
        encoder: str,
        speed: int,
        quality: int,
        threads: int,
    ) -> List[Dict]:
        """
        Process all XML and ssimulacra files in a single directory.

        Args:
            dir_path: Full path to directory
            files_info: Dictionary with "xml" and "ssimulacra" file lists
            encoder, speed, quality, threads: Parsed configuration values

        Returns:
            List of metric dictionaries for each image
        """
        results = []
        processed_images = set()

        # First, process XML files and their corresponding ssimulacra files
        for xml_file in files_info["xml"]:
            image_name = os.path.splitext(xml_file)[0]
            if image_name in processed_images:
                continue

            xml_path = os.path.join(dir_path, xml_file)
            xml_metrics = self._read_xml_file(xml_path)

            if xml_metrics is not None:
                metrics = {
                    "image": image_name,
                    "encoder": encoder,
                    "speed": speed,
                    "quality": quality,
                    "threads": threads,
                    **xml_metrics,
                }

                # Check for corresponding ssimulacra file
                ssimulacra_file = f"{image_name}.ssimulcra"
                if ssimulacra_file in files_info["ssimulacra"]:
                    ssimulacra_path = os.path.join(dir_path, ssimulacra_file)
                    ssimulacra_value = self._read_ssimulacra_file(ssimulacra_path)
                    if ssimulacra_value is not None:
                        metrics["ssimulacra2"] = ssimulacra_value

                results.append(metrics)
                processed_images.add(image_name)

        # Then, process standalone ssimulacra files (images without XML)
        for ssimulacra_file in files_info["ssimulacra"]:
            image_name = os.path.splitext(ssimulacra_file)[0]
            if image_name in processed_images:
                continue

            ssimulacra_path = os.path.join(dir_path, ssimulacra_file)
            ssimulacra_value = self._read_ssimulacra_file(ssimulacra_path)

            if ssimulacra_value is not None:
                metrics = {
                    "image": image_name,
                    "encoder": encoder,
                    "speed": speed,
                    "quality": quality,
                    "threads": threads,
                    "ssimulacra2": ssimulacra_value,
                }

                results.append(metrics)
                processed_images.add(image_name)

        return results

    def read_qm_logs_from_dir(self, qm_dir: str) -> pd.DataFrame:
        """
        Read quality metrics from XML files and ssimulacra result files in qm_dir.

        Args:
            qm_dir: Directory containing quality metric logs

        Returns:
            DataFrame with one row per image+encoder+quality_param+speed containing
            the quality metrics extracted from the logs.
        """
        # Collect all files by directory
        files_by_dir = self._collect_files_by_directory(qm_dir)

        if not files_by_dir:
            print("No quality metric files found in qm_dir")
            return pd.DataFrame()

        results = []

        # Process each directory
        for dir_path, files_info in files_by_dir.items():
            # Parse directory structure to extract encoder settings
            encoder, speed, quality, threads = self._parse_directory_path(
                files_info["rel_path"]
            )

            if encoder is None or quality is None or threads is None:
                continue

            # Process all files in this directory
            dir_results = self._process_files_in_directory(
                dir_path, files_info, encoder, speed, quality, threads
            )
            results.extend(dir_results)

        if not results:
            print("No quality metrics found in qm_dir")
            return pd.DataFrame()

        df = pd.DataFrame(results)
        print(f"Successfully loaded {len(df)} quality metric records from {qm_dir}")
        return df

    def get_encoder_extension(self, encoder: str) -> Optional[str]:
        """
        Get the correct file extension for the given encoder from config.

        Args:
            encoder: Name of the encoder

        Returns:
            File extension with dot (e.g., ".jpg") or None if not found
        """
        # Look through all codec types to find which one contains this encoder
        for _, codec_settings in self.common_settings.items():
            if encoder in codec_settings:
                # Get the encode_extension for this codec type
                encode_extension = codec_settings.get("encode_extension")
                if encode_extension:
                    # Ensure extension starts with dot
                    if not encode_extension.startswith("."):
                        encode_extension = "." + encode_extension
                    return encode_extension
        return None

    def _find_codec_type_for_encoder(self, encoder: str) -> Optional[str]:
        """
        Find the codec type that contains the given encoder.

        Args:
            encoder: Name of the encoder

        Returns:
            Codec type string or None if not found
        """
        for codec_type, codec_settings in self.common_settings.items():
            for encoder_name in codec_settings.keys():
                if encoder_name == encoder:
                    return codec_type
        return None

    def _build_encoded_directory_path(
        self, encoder: str, speed: int, quality: int, threads: int
    ) -> str:
        """
        Build the expected encoded file directory path using config or fallback patterns.

        Args:
            encoder, speed, quality, threads: Encoding parameters

        Returns:
            Relative directory path for encoded files
        """
        codec_type = self._find_codec_type_for_encoder(encoder)

        assert codec_type is not None, f"codec_type not found for '{encoder}'"

        # Use config-based directory format
        dir_format = self.common_settings[codec_type].get("dir_format", {})
        speed_suffix = dir_format.get("speed_suffix", "")
        quality_param = dir_format.get("quality_param", "q")

        # Build directory path using config format
        if speed_suffix:
            name_suffix = speed_suffix.format(speed=speed)
        else:
            name_suffix = ""

        return f"{encoder}{name_suffix}/{quality_param}{quality}_t{threads}"

    def _find_encoded_file_in_directory(
        self, dir_path: str, image: str, encoder: str
    ) -> Optional[float]:
        """
        Find and return the file size of the encoded file for a given image in a directory.

        Args:
            dir_path: Directory path to search in
            image: Image name (without extension)
            encoder: Encoder name for determining extension

        Returns:
            File size in MB or None if file not found
        """
        if not os.path.exists(dir_path):
            return None

        # First, try to find the file with the correct extension
        extension = self.get_encoder_extension(encoder)

        if extension:
            encoded_file = os.path.join(dir_path, f"{image}{extension}")
            if os.path.exists(encoded_file):
                file_size_bytes = os.path.getsize(encoded_file)
                return file_size_bytes / BYTES_IN_MB
            else:
                print(f"Warning: Could not find encoded file {encoded_file}")

        # If we can't find with expected extension, search for any file with this image name
        for file in os.listdir(dir_path):
            if file.startswith(image + "."):
                encoded_file = os.path.join(dir_path, file)
                file_size_bytes = os.path.getsize(encoded_file)
                return file_size_bytes / BYTES_IN_MB

        print(
            f"Warning: Could not find any encoded file for image '{image}' in {dir_path}"
        )
        return None

    def _get_file_size_for_row(self, row: pd.Series) -> float:
        """
        Get file size for a single row from the QM dataframe.

        Args:
            row: Single row from QM dataframe

        Returns:
            File size in MB or NaN if not found
        """
        encoder = row["encoder"]
        speed = row["speed"]
        quality = row["quality"]
        threads = row["threads"]
        image = row["image"]

        # Build the expected directory path
        encoded_subdir = self._build_encoded_directory_path(
            encoder, speed, quality, threads
        )
        encoded_dir_path = os.path.join(self.encoded_dir, encoded_subdir)

        # Find the encoded file and get its size
        file_size_mb = self._find_encoded_file_in_directory(
            encoded_dir_path, image, encoder
        )

        return file_size_mb if file_size_mb is not None else float("nan")

    def add_file_size_from_disk(self, qm_df: pd.DataFrame) -> pd.DataFrame:
        """
        Add file size information to QM dataframe by finding encoded files on disk.

        Args:
            qm_df: DataFrame with quality metrics per image/encoder/quality/speed

        Returns:
            DataFrame with added file_size_mb column
        """
        print("Adding file size information from disk...")

        # Apply the file size lookup to each row
        qm_df_copy = qm_df.copy()
        qm_df_copy["file_size_mb"] = qm_df_copy.apply(
            self._get_file_size_for_row, axis=1
        )

        return qm_df_copy

    def _organize_columns(self, merged_df: pd.DataFrame) -> pd.DataFrame:
        """
        Reorder columns for better readability and sort data.

        Args:
            merged_df: DataFrame to organize

        Returns:
            DataFrame with organized columns and sorted rows
        """
        # Define preferred column order
        column_order = [
            "image",
            "encoder",
            "speed",
            "quality",
            "threads",
            "file_size_mb",
            "encode_time",
            "decode_time",
        ]

        # Add quality metrics columns (if they exist)
        qm_columns = [
            "ssimulacra2",
            "psnr_y",
            "psnr_cb",
            "psnr_cr",
            "ssim",
            "ms_ssim",
            "vmaf",
            "vmaf_neg",
        ]
        column_order += qm_columns

        # Keep only columns that are in the merged_df
        column_order = [col for col in column_order if col in merged_df.columns]

        # Add any remaining columns
        remaining_cols = [col for col in merged_df.columns if col not in column_order]
        column_order.extend(remaining_cols)

        # Reorder columns
        merged_df = merged_df.reindex(columns=column_order)

        # Sort by encoder, speed, quality, image for better organization
        available_sort_columns = [
            col
            for col in ["encoder", "speed", "quality", "image"]
            if col in merged_df.columns
        ]
        if available_sort_columns:
            merged_df = merged_df.sort_values(available_sort_columns).reset_index(
                drop=True
            )

        return merged_df

    def merge_enc_dec_qm_dfs(
        self, encoding_df: pd.DataFrame, decoding_qm_df: pd.DataFrame
    ) -> pd.DataFrame:
        """
        Merge per-image logs from encoding, and decoding/QM scripts.

        Args:
            enc_csv_path: Path to encoding CSV file
            dec_csv_path: Path to decoding CSV file

        Returns:
            DataFrame with merged per-image results
        """
        try:
            merged_df = self._merge_enc_dec_qm_dfs(encoding_df, decoding_qm_df)

            if merged_df.empty:
                print("✗ No data available after merging.")
                return pd.DataFrame()

            merged_df = self._organize_columns(merged_df)

        except Exception as e:
            print(f"✗ Failed to merge data: {e}")
            merged_df = pd.DataFrame()

        return merged_df

    def read_decode_qm_csv_data(self) -> pd.DataFrame | None:
        """Read decoding+QM CSV file and return DataFrame with per-image decoding+QM info."""
        if not os.path.exists(self.decoding_csv_file):
            print(f"✗ Decoding+QM CSV file not found: {self.decoding_csv_file}")
            return None

        df = DataReaders._safe_read_csv(self.decoding_csv_file)
        if df is None:
            return None
        df["image"] = df["image_file"].str.split(".").str[0]
        return df

    def read_encoding_csv_data(self) -> pd.DataFrame | None:
        """Read encoding CSV file and return DataFrame with per-image encoding info."""
        if not os.path.exists(self.encoding_csv_file):
            print(f"✗ Encoding CSV file not found: {self.encoding_csv_file}")
            return None

        df = DataReaders._safe_read_csv(self.encoding_csv_file)
        if df is None:
            return None

        # Extract image name from file path
        df["image"] = df["image_file"].str.split(".").str[0]

        # Calculate file size for each encoded file
        if "output_size" in df.columns:
            df["file_size_mb"] = df["output_size"] / BYTES_IN_MB
        else:
            df["file_size_mb"] = df["encoded_path"].apply(self._get_file_size_mb)

        # Select and reorder columns
        columns_to_keep = [
            "image",
            "encoder",
            "speed",
            "quality",
            "threads",
            "encode_time",
            "file_size_mb",
            "input_size",
            "output_size",
            "encoded_path",
        ]
        # Only keep columns that exist in the DataFrame
        columns_to_keep = [col for col in columns_to_keep if col in df.columns]
        df = df[columns_to_keep]

        return df

    def _merge_enc_dec_qm_dfs(
        self,
        encoding_df: pd.DataFrame,
        dec_qm_df: pd.DataFrame,
    ) -> pd.DataFrame:
        """Merge all DataFrames on common keys."""
        merge_keys = ["image", "encoder", "speed", "quality", "threads"]
        merged_df = encoding_df.copy()
        if not dec_qm_df.empty:
            merged_df = merged_df.merge(dec_qm_df, on=merge_keys, how="outer")
        return merged_df

    @staticmethod
    def read_preprocessed_per_image_data(fname):
        df = DataReaders._safe_read_csv(fname)
        if df is None:
            return pd.DataFrame()
        if "image" not in df.columns and "image_file" in df.columns:
            df["image"] = df["image_file"].str.split(".").str[0]
        return df
