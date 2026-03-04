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
from typing import Dict, List, Tuple

import matplotlib.pyplot as plt
import pandas as pd
from bd_rate_utils import compute_average_bd_rates, compute_bd_rates_per_image

from matplotlib.backends.backend_pdf import PdfPages

from matplotlib.figure import Figure
from tqdm import tqdm


colors = [
    "blue",
    "red",
    "green",
    "orange",
    "purple",
    "brown",
    "pink",
    "gray",
    "olive",
    "cyan",
]
markers = ["o", "s", "^", "D", "v", "<", ">", "p", "*", "h"]


def _validate_rd_plot_data(
    per_image_df: pd.DataFrame, quality_metrics: List[str]
) -> List[str]:
    """Validate data and return available metrics for RD plotting."""
    if "file_size_mb" not in per_image_df.columns:
        print(
            "Warning: file_size_mb column not found. Cannot create rate-distortion plots."
        )
        return []

    available_metrics = [
        metric for metric in quality_metrics if metric in per_image_df.columns
    ]

    if not available_metrics:
        print("Warning: No quality metrics found in per-image data for plotting.")

    return available_metrics


def _get_datasets(per_image_df: pd.DataFrame) -> List[str]:
    """Get unique datasets from the DataFrame."""
    if "dataset" in per_image_df.columns:
        return list(per_image_df["dataset"].unique())
    return ["default"]


def _filter_dataset_data(per_image_df: pd.DataFrame, dataset: str) -> pd.DataFrame:
    """Filter DataFrame for a specific dataset."""
    if "dataset" in per_image_df.columns:
        return per_image_df[per_image_df["dataset"] == dataset]
    return per_image_df


def _create_single_rd_plot(image_data: pd.DataFrame, image: str, metric: str) -> Figure:
    """Create a single rate-distortion plot for an image and metric."""
    fig, ax = plt.subplots(figsize=(10, 6))

    # Group by encoder and speed combination
    i = 0
    for (encoder, speed), group in image_data.groupby(["encoder", "speed"]):
        if len(group) > 1:  # Only plot if we have multiple points
            # Sort by quality for proper line connection
            group_sorted = group.sort_values("quality")

            # Plot line with markers
            color = colors[i % len(colors)]
            marker = markers[(i // len(colors)) % len(markers)]
            label = f"{encoder} (speed={speed})"
            ax.plot(
                group_sorted["file_size_mb"],
                group_sorted[metric],
                color=color,
                marker=marker,
                label=label,
                linewidth=2,
                markersize=6,
            )
            i += 1

    # Customize plot
    ax.set_xlabel("File Size (MB)", fontsize=12)
    ax.set_ylabel(metric, fontsize=12)
    ax.set_title(
        f"Rate-Distortion Curve: {image} - {metric}",
        fontsize=14,
        fontweight="bold",
    )
    ax.grid(True, alpha=0.3)
    ax.legend(loc="lower right", fontsize=12)

    # Adjust layout to prevent legend cutoff
    plt.tight_layout()

    return fig


def create_per_image_rd_plots(
    per_image_df: pd.DataFrame, quality_metrics: List[str], output_dir: str
) -> None:
    """
    Create rate-distortion plots for all images, datasets, and metrics in a single unified PDF.

    Args:
        per_image_df: DataFrame with per-image results for all datasets
        quality_metrics: List of quality metrics to plot
        output_dir: Directory to save the plots
    """
    available_metrics = _validate_rd_plot_data(per_image_df, quality_metrics)
    if not available_metrics:
        return

    datasets = _get_datasets(per_image_df)
    unified_pdf_path = os.path.join(output_dir, "unified_rd_curves.pdf")
    total_operations = len(available_metrics) * per_image_df["image"].nunique()

    with PdfPages(unified_pdf_path) as pdf:
        with tqdm(total=total_operations, desc="Processing files") as pbar:
            for dataset in datasets:
                dataset_data = _filter_dataset_data(per_image_df, dataset)
                if dataset_data.empty:
                    continue

                dataset_images = dataset_data["image"].unique()
                for image in dataset_images:
                    for metric in available_metrics:
                        # Filter and clean data for this image
                        image_data = dataset_data[dataset_data["image"] == image].copy()
                        image_data = image_data.dropna(subset=["file_size_mb", metric])

                        if image_data.empty:
                            continue

                        # Create and save plot
                        fig = _create_single_rd_plot(image_data, image, metric)
                        pdf.savefig(fig, bbox_inches="tight")
                        plt.close(fig)

                        pbar.update(1)

    print(f"✓ RD curves saved to: {unified_pdf_path}")


def add_summary_table_to_pdf(
    pdf: PdfPages,
    summary_avg_bd_rates_df: pd.DataFrame,
    anchor_encoder: str,
    anchor_speed: int,
) -> None:
    """
    Add a summary BD-rate table as a page to the PDF.

    Args:
        pdf: PdfPages object to save the figure to
        summary_avg_bd_rates_df: DataFrame with summary BD rates to display as table
        anchor_encoder: Reference encoder name
        anchor_speed: Reference encoder speed
    """
    fig, ax = plt.subplots(figsize=(12, 8))
    ax.axis("tight")
    ax.axis("off")

    # Create table from DataFrame
    table_data = []

    # Add header row with metric names
    header = ["Encoder/Speed"] + list(summary_avg_bd_rates_df.columns)
    table_data.append(header)

    # Add data rows
    for idx, row in summary_avg_bd_rates_df.iterrows():
        encoder, speed = idx if isinstance(idx, tuple) else (idx, "")
        row_label = f"{encoder} s{speed}" if speed != "" else str(encoder)
        row_data = [f"{v:.2f}%" if pd.notna(v) else "N/A" for v in row.values]
        table_data.append([row_label] + row_data)

    # Create matplotlib table
    table = ax.table(
        cellText=table_data[1:],  # Data rows
        colLabels=table_data[0],  # Header row
        cellLoc="center",
        bbox=[0, 0, 1, 1],  # type: ignore
    )

    # Style the table
    table.auto_set_font_size(False)
    table.set_fontsize(10)
    table.scale(1, 2)

    # Style header row
    for i in range(len(header)):
        table[(0, i)].set_facecolor("#4CAF50")
        table[(0, i)].set_text_props(weight="bold", color="white")
        table[(0, i)].set_height(0.08)

    # Style data rows with alternating colors
    for i in range(1, len(table_data)):
        for j in range(len(header)):
            table[(i, j)].set_facecolor(["#EEEEEE", "white"][i % 2])
            table[(i, j)].set_height(0.06)

    # Add title
    ax.set_title(
        f"BD-Rate Summary Table\n(vs {anchor_encoder} speed {anchor_speed})",
        fontsize=16,
        fontweight="bold",
        pad=20,
    )

    plt.tight_layout()
    pdf.savefig(fig, bbox_inches="tight", facecolor="white")
    plt.close(fig)


def add_perf_table_to_pdf(
    pdf: PdfPages,
    summary_avg_perf_df: pd.DataFrame,
) -> None:
    """
    Add a performance table as a page to the PDF.

    Args:
        pdf: PdfPages object to save the figure to
        summary_avg_perf_df: DataFrame with summary performance to display as table
    """
    fig, ax = plt.subplots(figsize=(12, 8))
    ax.axis("tight")
    ax.axis("off")

    # Create table from DataFrame
    table_data = []

    # Add header row with metric names
    header = ["Encoder/Speed"] + list(summary_avg_perf_df.columns)
    table_data.append(header)

    # Add data rows
    for idx, row in summary_avg_perf_df.iterrows():
        encoder, speed = idx if isinstance(idx, tuple) else (idx, "")
        row_label = f"{encoder} s{speed}" if speed != "" else str(encoder)
        row_data = [f"{v:.5f}s" if pd.notna(v) else "N/A" for v in row.values]
        table_data.append([row_label] + row_data)

    # Create matplotlib table
    table = ax.table(
        cellText=table_data[1:],  # Data rows
        colLabels=table_data[0],  # Header row
        cellLoc="center",
        bbox=[0, 0, 1, 1],  # type: ignore
    )

    # Style the table
    table.auto_set_font_size(False)
    table.set_fontsize(10)
    table.scale(1, 2)

    # Style header row
    for i in range(len(header)):
        table[(0, i)].set_facecolor("#4CAF50")
        table[(0, i)].set_text_props(weight="bold", color="white")
        table[(0, i)].set_height(0.08)

    # Style data rows with alternating colors
    for i in range(1, len(table_data)):
        for j in range(len(header)):
            table[(i, j)].set_facecolor(["#EEEEEE", "white"][i % 2])
            table[(i, j)].set_height(0.06)

    # Add title
    ax.set_title(
        "Performance Summary Table",
        fontsize=16,
        fontweight="bold",
        pad=20,
    )

    plt.tight_layout()
    pdf.savefig(fig, bbox_inches="tight", facecolor="white")
    plt.close(fig)


def create_bd_rate_plots_pdf(
    metric_results: Dict[str, pd.DataFrame],
    anchor_encoder: str,
    anchor_speed: int,
    pdf_output_path: str,
    summary_avg_bd_rates_df: pd.DataFrame | None = None,
    summary_avg_perf_df: pd.DataFrame | None = None,
) -> None:
    """
    Create a single multi-page PDF with BD-rate vs encoding time plots for all metrics.

    Args:
        metric_results: Dictionary mapping metric names to their average BD-rate DataFrames
        anchor_encoder: Reference encoder name
        anchor_speed: Reference encoder speed
        pdf_output_path: Path to save the multi-page PDF
        summary_avg_bd_rates_df: Optional DataFrame with summary BD rates to display as table on first page
    """
    with PdfPages(pdf_output_path) as pdf:
        # Add summary table as first page if provided
        if summary_avg_bd_rates_df is not None:
            add_summary_table_to_pdf(
                pdf, summary_avg_bd_rates_df, anchor_encoder, anchor_speed
            )
        if summary_avg_perf_df is not None:
            add_perf_table_to_pdf(pdf, summary_avg_perf_df)

        allow_plot_without_timing = False

        for metric, avg_bd_rate_df in metric_results.items():
            try:
                if avg_bd_rate_df.empty:
                    print(f"    No valid BD-rate data for {metric}, skipping plot")
                    continue

                # Check if encoding time data is available
                if "avg_encode_time" not in avg_bd_rate_df.columns:
                    print(
                        f"    No encoding time data available for {metric}, skipping plot"
                    )
                    if allow_plot_without_timing:
                        avg_bd_rate_df["avg_encode_time"] = 50  # arbitrary value
                    else:
                        continue

                # Fill missing avg_encode_time values with increasing max+offsets
                max_time = avg_bd_rate_df["avg_encode_time"].max()
                dashed_line_pos = max_time + 50

                have_no_runtime_codecs = False
                for idx, ((encoder, speed), _) in enumerate(
                    avg_bd_rate_df.groupby(["encoder", "speed"])
                ):
                    fill_value = dashed_line_pos + 50 + 10 * idx
                    mask = (
                        (avg_bd_rate_df["encoder"] == encoder)
                        & (avg_bd_rate_df["speed"] == speed)
                        & (avg_bd_rate_df["avg_encode_time"].isna())
                    )
                    have_no_runtime_codecs = have_no_runtime_codecs or mask.any()
                    avg_bd_rate_df.loc[mask, "avg_encode_time"] = fill_value

                # Create the plot for this metric
                fig, ax = plt.subplots(figsize=(12, 8))

                # Add a dashed line to separate encoding with time and those without time
                if have_no_runtime_codecs:
                    ax.axvline(
                        x=dashed_line_pos,
                        color="black",
                        linestyle="--",
                        linewidth=2,
                        alpha=0.7,
                        zorder=2,
                    )

                # Get unique encoders
                encoders = avg_bd_rate_df["encoder"].unique()

                for i, encoder in enumerate(encoders):
                    encoder_data = avg_bd_rate_df[
                        avg_bd_rate_df["encoder"] == encoder
                    ].copy()

                    # Filter out entries with missing encoding time data
                    encoder_data = encoder_data.dropna(subset=["avg_encode_time"])

                    if encoder_data.empty:
                        continue

                    # Sort by encoding time for proper line connection
                    encoder_data = encoder_data.sort_values("avg_encode_time")

                    color = colors[i % len(colors)]
                    marker = markers[(i // len(colors)) % len(markers)]

                    # Plot points and connect with lines if multiple points
                    ax.plot(
                        encoder_data["avg_encode_time"],
                        encoder_data["avg_bd_rate"],
                        color=color,
                        marker=marker,
                        linewidth=2.5,
                        markersize=10,
                        label=encoder,
                        zorder=5,
                        markeredgecolor="black",
                        markeredgewidth=1,
                    )

                    # Add speed/preset labels on points
                    for _, row in encoder_data.iterrows():
                        # Position label to avoid overlap
                        x_offset = 15 if row["avg_bd_rate"] >= 0 else -25
                        y_offset = 15

                        ax.annotate(
                            row["speed"],
                            (row["avg_encode_time"], row["avg_bd_rate"]),
                            xytext=(x_offset, y_offset),
                            textcoords="offset points",
                            ha="center",
                            va="center",
                            fontsize=11,
                            fontweight="bold",
                            color=color,
                            bbox={
                                "boxstyle": "round,pad=0.2",
                                "facecolor": "white",
                                "alpha": 0.9,
                                "edgecolor": color,
                                "linewidth": 1,
                            },
                        )

                # Customize the plot
                ax.set_xlabel("Avg Time (s)", fontsize=14, fontweight="bold")
                ax.set_ylabel("Avg BD Rate (%)", fontsize=14, fontweight="bold")
                ax.set_title(
                    f"BD-rate vs Encoding Time - {metric}\n(vs {anchor_encoder} speed {anchor_speed})",
                    fontsize=16,
                    fontweight="bold",
                    pad=20,
                )
                ax.grid(True, alpha=0.4, linestyle="--")
                ax.legend(loc="upper right", fontsize=12)

                # Add horizontal line at y=0 for reference
                ax.axhline(
                    y=0, color="black", linestyle="-", alpha=0.8, zorder=1, linewidth=1
                )

                # Use log scale on x-axis
                ax.set_xscale("log")

                # Improve tick formatting
                ax.tick_params(axis="both", which="major", labelsize=12)

                # Adjust layout to prevent legend cutoff
                plt.tight_layout()

                # Save this page to PDF
                pdf.savefig(fig, bbox_inches="tight", facecolor="white")
                plt.close(fig)

            except Exception as e:
                print(f"    ✗ Error creating plot for {metric}: {e}")
                continue

    print(f"✓ BD-rate plots PDF saved to: {pdf_output_path}")


def _validate_bd_rate_config(settings: Dict) -> bool:
    """Validate BD-rate analysis configuration."""
    if not settings.get("allow_bdrate", False):
        print("BD-rate analysis is disabled in config (allow_bdrate=false)")
        return False
    return True


def _load_and_validate_data(per_image_csv_path: str) -> pd.DataFrame:
    """Load and validate per-image data for BD-rate analysis."""
    try:
        per_image_df = pd.read_csv(per_image_csv_path)
        if not per_image_df.empty:
            return per_image_df
        print("No per-image data available for BD-rate analysis")
    except Exception as e:
        print(f"Error reading per-image data for BD-rate analysis: {e}")

    return pd.DataFrame()


def _get_anchor_settings(per_image_df: pd.DataFrame, settings: Dict) -> tuple:
    """Get anchor encoder and speed settings."""
    anchor_encoder = settings.get("anchor_encoder")
    anchor_speed = settings.get("anchor_speed")

    # If no anchor specified in config, use first encoder/speed combination
    if not anchor_encoder or anchor_speed is None:
        anchor_encoder = per_image_df["encoder"].iloc[0]
        anchor_speed = per_image_df["speed"].iloc[0]
        print(
            f"No anchor specified in config for BD rate analysis, using first available: {anchor_encoder} speed {anchor_speed}"
        )
    else:
        print(
            f"Using anchor from config: {anchor_encoder} speed {anchor_speed} for BD rate analysis"
        )

    return anchor_encoder, anchor_speed


def _compute_bd_rates_for_metric(
    metric: str,
    per_image_csv_path: str,
    anchor_encoder: str,
    anchor_speed: int,
    metrics_dir: str,
    per_image_df: pd.DataFrame,
) -> tuple:
    """Compute BD-rates for a single metric and return processed DataFrames."""
    metric_dir = os.path.join(metrics_dir, metric.lower())
    os.makedirs(metric_dir, exist_ok=True)

    bd_rates_csv = os.path.join(metric_dir, "bd_rates_per_image.csv")
    avg_bd_rates_csv = os.path.join(metric_dir, "average_bd_rates.csv")

    # Compute BD-rates per image
    bd_rate_df = compute_bd_rates_per_image(
        per_image_csv_path=per_image_csv_path,
        anchor_encoder=anchor_encoder,
        anchor_speed=anchor_speed,
        output_csv_path=bd_rates_csv,
        quality_metric=metric,
    )

    bd_rate_df_copy = None
    if not bd_rate_df.empty:
        bd_rate_df_copy = bd_rate_df.copy()
        bd_rate_df_copy["metric"] = metric
        # Add dataset column if it exists in the original data
        if "dataset" in per_image_df.columns:
            # Map image names to datasets
            image_to_dataset = per_image_df.set_index("image")["dataset"].to_dict()
            bd_rate_df_copy["dataset"] = bd_rate_df_copy["image"].map(image_to_dataset)
        else:
            bd_rate_df_copy["dataset"] = "default"

    # Compute average BD-rates and encoding times
    avg_bd_rate_df = compute_average_bd_rates(
        bd_rate_df=bd_rate_df,
        per_image_csv_path=per_image_csv_path,
        output_csv_path=avg_bd_rates_csv,
    )

    avg_bd_rate_df_copy = None
    if not avg_bd_rate_df.empty:
        avg_bd_rate_df_copy = avg_bd_rate_df.copy()
        avg_bd_rate_df_copy["metric"] = metric
        if "dataset" in per_image_df.columns:
            # For average BD rates, we'll use "all_datasets" since it's averaged across images
            avg_bd_rate_df_copy["dataset"] = "all_datasets"
        else:
            avg_bd_rate_df_copy["dataset"] = "default"
    else:
        print(f"✗ No valid BD-rate data for {metric}")

    return bd_rate_df_copy, avg_bd_rate_df_copy, avg_bd_rate_df


def _create_unified_csv_files(
    all_bd_rates_per_image: List[pd.DataFrame],
    all_avg_bd_rates: List[pd.DataFrame],
    output_dir: str,
) -> Tuple[pd.DataFrame | None, pd.DataFrame | None]:
    """Create unified BD-rate CSV files."""
    if all_bd_rates_per_image:
        # Create unified per-image BD rates CSV
        unified_bd_rates_per_image_df = pd.concat(
            all_bd_rates_per_image, axis=0, ignore_index=True
        )
        unified_bd_rates_per_image_csv = os.path.join(
            output_dir, "unified_bd_rates_per_image.csv"
        )
        unified_bd_rates_per_image_df.to_csv(
            unified_bd_rates_per_image_csv, index=False
        )
        print(
            f"✓ Unified per-image BD rates CSV saved to: {unified_bd_rates_per_image_csv}"
        )
    summary_avg_bd_rates_df = None
    summary_avg_perf_df = None
    if all_avg_bd_rates:
        # Create unified average BD rates CSV
        unified_avg_bd_rates_df = pd.concat(all_avg_bd_rates, axis=0, ignore_index=True)
        unified_avg_bd_rates_csv = os.path.join(
            output_dir, "unified_average_bd_rates.csv"
        )
        unified_avg_bd_rates_df.to_csv(unified_avg_bd_rates_csv, index=False)
        print(f"✓ Unified average BD rates CSV saved to: {unified_avg_bd_rates_csv}")
        summary_avg_bd_rates_df = unified_avg_bd_rates_df.pivot_table(
            index=["encoder", "speed"], columns=["quality_metric"], values="avg_bd_rate"
        )
        summary_avg_bd_rates_df["avg"] = summary_avg_bd_rates_df.drop(
            columns=["psnr_cb", "psnr_cr"],
            errors="ignore",
        ).mean(axis=1)
        cols = ["encoder", "speed", "avg_encode_time", "avg_decode_time"]
        summary_avg_perf_df = unified_avg_bd_rates_df.drop_duplicates(
            subset=["encoder", "speed"]
        )[cols]
        summary_avg_perf_df.set_index(["encoder", "speed"], inplace=True)
        summary_avg_bd_rates_csv = os.path.join(
            output_dir, "summary_average_bd_rates.csv"
        )
        summary_avg_perf_csv = os.path.join(output_dir, "summary_average_perf.csv")
        summary_avg_bd_rates_df.to_csv(summary_avg_bd_rates_csv, index=True)
        print(f"✓ Summary average BD rates CSV saved to: {summary_avg_bd_rates_csv}")
        summary_avg_perf_df.to_csv(summary_avg_perf_csv, index=True)
        print(f"✓ Summary average perf CSV saved to: {summary_avg_perf_csv}")
    return summary_avg_bd_rates_df, summary_avg_perf_df


def run_bd_rate_analysis(
    per_image_csv_path: str, quality_metrics: List[str], output_dir: str, settings: Dict
) -> None:
    """
    Run BD-rate analysis for all available quality metrics if enabled in config.

    Args:
        per_image_csv_path: Path to the per_image_results.csv file
        quality_metrics: List of quality metrics to analyze
        output_dir: Directory to save results
        settings: Configuration settings dictionary
    """
    if not _validate_bd_rate_config(settings):
        return

    per_image_df = _load_and_validate_data(per_image_csv_path)
    if per_image_df.empty:
        return

    anchor_encoder, anchor_speed = _get_anchor_settings(per_image_df, settings)

    # Determine available quality metrics
    available_metrics = _validate_rd_plot_data(per_image_df, quality_metrics)

    if not available_metrics:
        print("No quality metrics found in per-image data for BD-rate analysis")
        return

    # Create metrics directory structure
    metrics_dir = os.path.join(output_dir, "metrics")
    os.makedirs(metrics_dir, exist_ok=True)

    # Dictionary to collect average BD-rate results for PDF generation
    metric_results = {}

    # Lists to collect unified BD-rate data
    all_bd_rates_per_image = []
    all_avg_bd_rates = []

    # Run BD-rate analysis for each metric and save individual CSVs
    for metric in available_metrics:
        try:
            bd_rate_df_copy, avg_bd_rate_df_copy, avg_bd_rate_df = (
                _compute_bd_rates_for_metric(
                    metric,
                    per_image_csv_path,
                    anchor_encoder,
                    anchor_speed,
                    metrics_dir,
                    per_image_df,
                )
            )

            if bd_rate_df_copy is not None:
                all_bd_rates_per_image.append(bd_rate_df_copy)

            if avg_bd_rate_df_copy is not None:
                all_avg_bd_rates.append(avg_bd_rate_df_copy)
                metric_results[metric] = avg_bd_rate_df

        except Exception as e:
            print(f"✗ Error during BD-rate analysis for {metric}: {e}")
            continue

    # Create unified BD-rate CSV files
    summary_bd_rates_df, summary_perf_df = _create_unified_csv_files(
        all_bd_rates_per_image, all_avg_bd_rates, output_dir
    )

    # Create single multi-page PDF for all BD-rate plots using collected results
    if metric_results:
        pdf_output_path = os.path.join(output_dir, "bd_rate_plots.pdf")
        create_bd_rate_plots_pdf(
            metric_results=metric_results,
            anchor_encoder=anchor_encoder,
            anchor_speed=anchor_speed,
            pdf_output_path=pdf_output_path,
            summary_avg_bd_rates_df=summary_bd_rates_df,
            summary_avg_perf_df=summary_perf_df,
        )
    else:
        print("No BD-rate results available for PDF generation")
