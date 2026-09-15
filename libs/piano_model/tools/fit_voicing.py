#!/usr/bin/env python3
"""Offline, empirical modal voicing of complete stereo piano renders. See VOICING.md."""

import argparse
import csv
from dataclasses import dataclass
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import struct
import sys
import tempfile

import numpy as np


KNOTS = (28, 68, 112)
REPRESENTATIVE_VELOCITIES = (13, 28, 35, 40, 45, 48, 53, 60, 68, 76, 84, 92, 100, 112, 116, 124)
PARTIALS = 240
TIMES = np.array([0.06, 0.35, 0.65, 1.0, 1.4])
WINDOW_SECONDS = np.array([0.12, 0.30, 0.30, 0.30, 0.30])
EARLY_SECONDS = 0.15
GAIN_LIMITS = (-36.0, 24.0)
GAIN_STEP = 12.0
DECAY_LIMITS = (0.1, 4.0)
DECAY_STEP = (0.25, 2.0)
PROBE_DECAY_STEP = (0.5, 2.0)
CSV_FIELDS = ("key", "partial", "pp_db", "mf_db", "ff_db", "decay_scale")
OUTPUTS = ("calibration.csv", "calibration_data.rs", "metadata.json", "summary.json")


def read_wav(path):
    """Read little-endian RIFF PCM16/24/32 or float32, including extensible WAV.

    Channels remain separate, as float64 in [-1, 1] for integer PCM. Float
    samples are not clipped. Reject damaged/unsupported files instead of guessing.
    """
    raw = Path(path).read_bytes()
    if len(raw) < 12 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise ValueError(f"{path}: expected little-endian RIFF/WAVE")
    end = struct.unpack_from("<I", raw, 4)[0] + 8
    if end > len(raw):
        raise ValueError(f"{path}: truncated RIFF")
    fmt, payload = None, None
    pos = 12
    while pos + 8 <= end:
        tag, size = struct.unpack_from("<4sI", raw, pos)
        pos += 8
        if pos + size > end:
            raise ValueError(f"{path}: truncated WAV chunk")
        if tag == b"fmt ":
            fmt = raw[pos:pos + size]
        elif tag == b"data":
            if payload is not None:
                raise ValueError(f"{path}: multiple data chunks unsupported")
            payload = raw[pos:pos + size]
        pos += size + (size & 1)
    if fmt is None or len(fmt) < 16 or payload is None:
        raise ValueError(f"{path}: missing fmt/data chunk")
    encoding, channels, rate, byte_rate, align, bits = struct.unpack_from("<HHIIHH", fmt)
    if encoding == 0xFFFE:
        if len(fmt) < 40 or struct.unpack_from("<H", fmt, 16)[0] < 22:
            raise ValueError(f"{path}: truncated extensible format")
        valid_bits = struct.unpack_from("<H", fmt, 18)[0]
        if valid_bits not in (0, bits) or fmt[28:40] != bytes.fromhex("00001000800000aa00389b71"):
            raise ValueError(f"{path}: unsupported extensible PCM layout")
        encoding = struct.unpack_from("<I", fmt, 24)[0]
    if channels not in (1, 2) or rate <= 0 or bits not in (16, 24, 32):
        raise ValueError(f"{path}: unsupported channels/rate/bit depth")
    if align != channels * (bits // 8) or byte_rate != rate * align or len(payload) % align:
        raise ValueError(f"{path}: invalid PCM block alignment")
    if encoding == 3 and bits == 32:
        samples = np.frombuffer(payload, dtype="<f4").astype(np.float64)
    elif encoding == 1 and bits == 24:
        octets = np.frombuffer(payload, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        integers = octets[:, 0] | (octets[:, 1] << 8) | (octets[:, 2] << 16)
        samples = ((integers ^ 0x800000) - 0x800000).astype(np.float64) / 8388608.0
    elif encoding == 1 and bits in (16, 32):
        samples = np.frombuffer(payload, dtype=f"<i{bits // 8}").astype(np.float64) / 2 ** (bits - 1)
    else:
        raise ValueError(f"{path}: unsupported WAV encoding {encoding}/{bits}")
    samples = samples.reshape(-1, channels)
    if not samples.size or not np.isfinite(samples).all():
        raise ValueError(f"{path}: empty or non-finite audio")
    return samples, rate, {"encoding": encoding, "bits": bits, "channels": channels}


def onset_index(samples, rate):
    """First 1 ms stereo RMS block above -40 dB relative to first-0.5s peak."""
    block = max(1, round(rate * 0.001))
    count = min(len(samples), round(rate * 0.5)) // block
    if not count:
        raise ValueError("silent onset: audio shorter than one RMS block")
    power = np.mean(samples[:count * block].reshape(count, block, -1) ** 2, axis=(1, 2))
    if np.max(power) <= 1e-20:
        raise ValueError("silent onset: no signal in first 0.5 seconds")
    return int(np.flatnonzero(power > np.max(power) * 1e-4)[0] * block)


def power_spectrum(samples, rate, nfft=None):
    """One-sided stereo mean-square POWER per bin, with Parseval/Hann scaling."""
    size = len(samples)
    if size < 4:
        raise ValueError("FFT window too short")
    if nfft is None:
        nfft = 2 * (1 << (size - 1).bit_length())
    if nfft < size:
        raise ValueError("FFT size cannot truncate window")
    window = np.hanning(size)
    spectrum = np.fft.rfft(samples * window[:, None], n=nfft, axis=0)
    power = np.mean(np.abs(spectrum) ** 2, axis=1) / (nfft * np.sum(window ** 2))
    power[1:-1 if nfft % 2 == 0 else None] *= 2.0
    return np.fft.rfftfreq(nfft, 1.0 / rate), power


def midi_frequency(key):
    return 440.0 * 2.0 ** ((key - 69) / 12.0)


def partial_frequencies(f0, stiffness, count=PARTIALS):
    n = np.arange(1, count + 1, dtype=float)
    return f0 * n * np.sqrt(1.0 + stiffness * n * n)


def weighted_median(values, weights):
    order = np.argsort(values, kind="stable")
    values, weights = np.asarray(values)[order], np.asarray(weights)[order]
    cumulative = np.cumsum(weights)
    half = np.sum(weights) * 0.5
    tolerance = np.sum(weights) * 1e-12
    index = int(np.searchsorted(cumulative, half - tolerance))
    # Resolve an exactly balanced pair symmetrically, including roundoff. Otherwise
    # a constant dB shift can flip which of two slope estimates is the median.
    if index + 1 < len(values) and abs(cumulative[index] - half) <= tolerance:
        return float((values[index] + values[index + 1]) / 2)
    return float(values[index])


def robust_average(values, weights):
    values, weights = np.asarray(values), np.asarray(weights)
    center = weighted_median(values, weights)
    scale = max(0.05, 1.4826 * weighted_median(np.abs(values - center), weights))
    adjusted = weights * np.minimum(1.0, 1.5 * scale / np.maximum(np.abs(values - center), 1e-12))
    return float(np.sum(values * adjusted) / np.sum(adjusted))


def spectral_peaks(freq, power, duration):
    """Prominent local lines only; noise does not become a candidate harmonic."""
    candidates = np.flatnonzero((power[1:-1] > power[:-2]) & (power[1:-1] >= power[2:])) + 1
    candidates = candidates[power[candidates] > np.max(power) * 1e-6]
    found, strengths = [], []
    for index in candidates:
        # Exclude the whole close-unison cluster when estimating the floor.
        # A median through its center mistakes neighboring strings for noise.
        radius = max(14.0 / duration, freq[index] * 0.024)
        exclusion = max(4.0 / duration, freq[index] * 0.010)
        lo, hi = np.searchsorted(freq, [freq[index] - radius, freq[index] + radius])
        background = power[lo:hi][abs(freq[lo:hi] - freq[index]) > exclusion]
        if not len(background):
            continue
        floor = max(float(np.median(background)), np.max(power) * 1e-12, 1e-30)
        prominence = 10.0 * np.log10(power[index] / floor)
        if prominence < 15.0:
            continue
        logs = np.log(np.maximum(power[index - 1:index + 2], 1e-30))
        denominator = logs[0] - 2.0 * logs[1] + logs[2]
        delta = np.clip(0.5 * (logs[0] - logs[2]) / min(denominator, -1e-15), -0.5, 0.5)
        found.append(float(freq[index] + delta * (freq[1] - freq[0])))
        relative_db = 10.0 * np.log10(power[index] / np.max(power))
        strengths.append(float(np.clip((prominence - 15.0) / 15.0, 0, 1)
                               * np.clip((relative_db + 60.0) / 20.0, 0, 1)))
    return np.asarray(found), np.asarray(strengths)


def early_partial_lines(samples, rate, predicted, gaps):
    """Match early local lines to a fixed, independently fitted long-FFT curve."""
    segment = samples[:round(EARLY_SECONDS * rate)]
    freq, power = power_spectrum(segment, rate)
    duration = len(segment) / rate
    maximum = max(float(np.max(power)), 1e-30)
    peaks = np.flatnonzero((power[1:-1] > power[:-2]) & (power[1:-1] >= power[2:])) + 1
    centers, confidence = predicted.copy(), np.zeros_like(predicted)
    used = set()
    for p, center in enumerate(predicted):
        # Disjoint search bands and explicit peak ownership prevent aliases.
        tolerance = min(gaps[p] * 0.23, max(2.0 / duration, center * 0.003))
        radius = min(gaps[p] * 0.23, 2.5 * tolerance)
        if center + radius >= rate * 0.49:
            continue
        candidates = peaks[abs(freq[peaks] - center) <= radius]
        flank = (abs(freq - center) < gaps[p] * 0.45) & (abs(freq - center) > radius * 1.15)
        if not len(candidates) or np.count_nonzero(flank) < 4:
            continue
        floor = max(float(np.median(power[flank])), maximum * 1e-12, 1e-30)
        best = None
        for index in candidates:
            if int(index) in used:
                continue
            prominence = 10 * math.log10(max(power[index] / floor, 1e-30))
            relative = 10 * math.log10(max(power[index] / maximum, 1e-30))
            if prominence <= 15 or relative <= -60:
                continue
            logs = np.log(np.maximum(power[index - 1:index + 2], 1e-30))
            denominator = logs[0] - 2 * logs[1] + logs[2]
            delta = np.clip(0.5 * (logs[0] - logs[2]) / min(denominator, -1e-15), -0.5, 0.5)
            observed = float(freq[index] + delta * (freq[1] - freq[0]))
            if abs(observed - center) > radius:
                continue
            proximity = math.exp(-0.5 * ((observed - center) / tolerance) ** 2)
            quality = float(np.clip((prominence - 15) / 15, 0, 1)
                            * np.clip((relative + 60) / 20, 0, 1) * proximity)
            if best is None or quality > best[0]:
                best = quality, observed, int(index)
        if best is not None:
            confidence[p], centers[p], index = best
            used.add(index)
    return centers, confidence


def identify_partials(samples, rate, key):
    """Independent constrained stiff-string fit, then measured line neighborhoods.

    The stiff-string curve identifies harmonic numbers, not production frequencies.
    Local measured centers (including an independent p1) are retained for analysis.
    """
    segment = samples[round(0.1 * rate):round(1.2 * rate)]
    freq, power = power_spectrum(segment, rate)
    duration = len(segment) / rate
    peaks, strength = spectral_peaks(freq, power, duration)
    nominal = midi_frequency(key)
    n = np.arange(1, PARTIALS + 1, dtype=float)

    def match(predicted):
        if not len(peaks):
            return np.zeros(predicted.shape, dtype=int), np.zeros_like(predicted)
        right = np.clip(np.searchsorted(peaks, predicted), 0, len(peaks) - 1)
        left = np.maximum(right - 1, 0)
        index = np.where(abs(peaks[left] - predicted) < abs(peaks[right] - predicted), left, right)
        tolerance = np.maximum(2.0 / duration, predicted * 0.003)
        distance = abs(peaks[index] - predicted) / tolerance
        confidence = strength[index] * np.exp(-0.5 * distance ** 2) * (distance < 2.5)
        confidence *= predicted < rate * 0.48
        return index, confidence

    # Deterministic coarse grid avoids silently locking high harmonics to B=0.
    b_grid = np.r_[0.0, np.geomspace(1e-7, 0.01, 65)]
    f_grid = nominal * 2.0 ** (np.linspace(-45, 45, 37) / 1200.0)
    predicted = f_grid[:, None, None] * n * np.sqrt(1.0 + b_grid[None, :, None] * n * n)
    _, confidence = match(predicted)
    score = np.sum(confidence / np.sqrt(n), axis=-1)
    fi, bi = np.unravel_index(np.argmax(score), score.shape)
    f0, stiffness = float(f_grid[fi]), float(b_grid[bi])
    if not len(peaks):
        f0, stiffness = nominal, 0.0
    for _ in range(6):
        predicted = partial_frequencies(f0, stiffness)
        indices, confidence = match(predicted)
        valid = confidence > 0.15
        if np.count_nonzero(valid) < 3:
            break
        nv, measured = n[valid], peaks[indices[valid]]
        xscale = np.max(nv ** 2)
        design = np.column_stack([np.ones(len(nv)), nv ** 2 / xscale])
        weights = confidence[valid] / np.sqrt(nv)
        target = (measured / nv) ** 2
        for _ in range(4):
            root = np.sqrt(weights)
            coeff = np.linalg.lstsq(design * root[:, None], target * root, rcond=None)[0]
            fitted = design @ coeff
            residual = abs(target - fitted) / np.maximum(target, 1.0)
            weights = confidence[valid] / np.sqrt(nv) * np.minimum(1.0, 0.003 / np.maximum(residual, 1e-12))
        f0 = float(np.clip(math.sqrt(max(coeff[0], 1.0)), nominal * 2 ** (-45 / 1200), nominal * 2 ** (45 / 1200)))
        stiffness = float(np.clip(coeff[1] / xscale / (f0 * f0), 0.0, 0.01))
    predicted = partial_frequencies(f0, stiffness)
    indices, confidence = match(predicted)
    centers = predicted.copy()
    if len(peaks):
        centers[confidence > 0.1] = peaks[indices[confidence > 0.1]]
    supported = confidence > 0.25
    count = int(np.count_nonzero(supported))
    residual_cents = (1200 * np.log2(centers[supported] / predicted[supported]))
    error = float(np.sqrt(np.mean(residual_cents ** 2))) if count else 0.0
    fit_confidence = float(np.clip((count - 2) / 6, 0, 1) * math.exp(-(error / 8) ** 2))
    # With too few lines B is unidentified; do not extrapolate dubious upper modes.
    confidence[2:] *= 0.35 + 0.65 * fit_confidence
    gaps = np.minimum(np.diff(np.r_[0.0, predicted]), np.diff(np.r_[predicted, partial_frequencies(f0, stiffness, PARTIALS + 1)[-1]]))
    boundary = bool(stiffness >= 0.0099 or abs(1200 * math.log2(f0 / nominal)) >= 44.9)
    long_confidence = confidence.copy()
    early_centers, early_confidence = early_partial_lines(samples, rate, predicted, gaps)
    # Early peaks supply line evidence, never f0/B evidence. An unidentified or
    # boundary-constrained curve cannot assign new high partials reliably.
    early_confidence *= fit_confidence if not boundary and fit_confidence >= 0.25 else 0.0
    use_early = (confidence <= 0.25) & (early_confidence > confidence)
    centers[use_early] = early_centers[use_early]
    confidence[use_early] = early_confidence[use_early]
    return centers, confidence, gaps, {
        "f0_hz": f0, "B": stiffness, "confidence": fit_confidence,
        "supported_lines": count, "residual_cents_rms": error,
        "constraint_boundary": boundary,
        "long_line_confidence": long_confidence.tolist(),
        "early_line_confidence": early_confidence.tolist(),
        "early_line_used": use_early.tolist(),
    }


def robust_decay(times, power, confidence):
    """Positive empirical dB/s loss and confidence; reject rises, floors and beating."""
    good = (times >= 0.2) & (confidence > 0.2) & (power > 0)
    t, y, w = times[good], 10 * np.log10(np.maximum(power[good], 1e-30)), confidence[good]
    if len(t) < 3 or t[-1] - t[0] < 0.7:
        return 0.0, 0.0
    slopes, weights = [], []
    for i in range(len(t)):
        for j in range(i + 1, len(t)):
            slopes.append((y[j] - y[i]) / (t[j] - t[i]))
            weights.append(w[i] * w[j] * (t[j] - t[i]))
    slope = weighted_median(slopes, weights)
    intercept = weighted_median(y - slope * t, w)
    residual = abs(y - (intercept + slope * t))
    scatter = weighted_median(abs(np.asarray(slopes) - slope), weights)
    if slope >= -0.4 or np.max(np.diff(y)) > 2.0 or np.max(residual) > 3.0:
        return 0.0, 0.0
    quality = float(np.mean(w) * math.exp(-(float(np.sqrt(np.mean(residual ** 2))) / 1.5) ** 2)
                    / (1 + (scatter / (0.3 * abs(slope) + 0.3)) ** 2)
                    * np.clip((-slope - 0.4) / 1.5, 0, 1))
    return float(-slope), quality


@dataclass
class Measurement:
    power: np.ndarray
    confidence: np.ndarray
    power_upper_bound: np.ndarray
    location_confidence: np.ndarray
    loss: np.ndarray
    loss_confidence: np.ndarray
    times: np.ndarray
    window_samples: np.ndarray
    rate: int
    report: dict


def analyze(samples, rate, key):
    onset = onset_index(samples, rate)
    samples = samples[onset:]
    sizes = np.rint(WINDOW_SECONDS * rate).astype(int)
    starts = np.maximum(0, np.rint(TIMES * rate - sizes / 2).astype(int))
    if len(samples) < np.max(starts + sizes):
        raise ValueError("audio ends before final 1.4-second measurement window")
    centers, line_confidence, gaps, fit = identify_partials(samples, rate, key)
    predicted = partial_frequencies(fit["f0_hz"], fit["B"])
    location_confidence = np.zeros(PARTIALS)
    long_confidence = np.asarray(fit["long_line_confidence"])
    supported = np.flatnonzero(long_confidence > 0.25)
    if fit["confidence"] >= 0.25 and not fit["constraint_boundary"] and len(supported):
        # Interpolate only inside the independently identified harmonic range.
        location_confidence[supported[0]:supported[-1] + 1] = fit["confidence"]
        # A missing fundamental is located by at least three reliable low modes;
        # extrapolating down from p2..p8 is insensitive to high-mode stiffness.
        if np.count_nonzero(long_confidence[1:8] > 0.25) >= 3:
            location_confidence[0] = fit["confidence"]
        # An observed early high line supports its own location only; do not
        # extrapolate missing-reference ceilings through the intervening tail.
        early_supported = np.asarray(fit["early_line_used"]) & (line_confidence > 0.25)
        location_confidence[early_supported] = fit["confidence"] * line_confidence[early_supported]
    powers = np.zeros((len(TIMES), PARTIALS))
    confidences = np.zeros_like(powers)
    upper_bounds = np.zeros_like(powers)
    tracks = np.zeros_like(powers)
    times = np.zeros(len(TIMES))
    for ti, (start, size) in enumerate(zip(starts, sizes)):
        times[ti] = (start + (size - 1) / 2) / rate
        freq, power = power_spectrum(samples[start:start + size], rate)
        total = max(float(np.sum(power)), 1e-30)
        for p in range(PARTIALS):
            for bound, center in ((True, predicted[p]), (False, centers[p])):
                half_width = min(gaps[p] * 0.23, max(2.5 / (size / rate), center * 0.008))
                if (not bound and line_confidence[p] <= 0.05) or center + half_width >= rate * 0.49:
                    continue
                band = (freq >= center - half_width) & (freq <= center + half_width)
                flank = (abs(freq - center) < gaps[p] * 0.45) & (abs(freq - center) > half_width * 1.15)
                if np.count_nonzero(flank) < 4 or not np.any(band):
                    continue
                floor = max(float(np.median(power[flank])), total * 1e-12, 1e-30)
                noise = floor * np.count_nonzero(band)
                band_total = float(np.sum(power[band]))
                if bound:
                    # Keep all measured energy, including the floor, plus one
                    # extra local-floor allowance. Never interpret this as a line.
                    upper_bounds[ti, p] = band_total + noise
                    continue
                signal = max(band_total - noise, 0.0)
                snr = 10 * math.log10(max(signal / noise, 1e-30))
                relative = 10 * math.log10(max(signal / total, 1e-30))
                quality = np.clip((snr - 10) / 15, 0, 1) * np.clip((relative + 60) / 20, 0, 1)
                powers[ti, p] = signal
                confidences[ti, p] = quality * line_confidence[p]
                tracks[ti, p] = freq[band][np.argmax(power[band])]
    losses, loss_confidence = np.zeros(PARTIALS), np.zeros(PARTIALS)
    for p in range(PARTIALS):
        losses[p], loss_confidence[p] = robust_decay(times, powers[:, p], confidences[:, p])
    rms_slice = samples[round(0.05 * rate):round(0.45 * rate)]
    report = {"onset_samples": onset, "onset_seconds": onset / rate,
              "window_centers_seconds": times.tolist(), "window_samples": sizes.tolist(),
              "window_start_samples": starts.tolist(), "window_durations_seconds": (sizes / rate).tolist(),
              "pitch_fit": fit, "partial_centers_hz": centers.tolist(),
              "predicted_centers_hz": predicted.tolist(),
              "power_upper_bound": upper_bounds.tolist(),
              "location_confidence": location_confidence.tolist(),
              "tracked_peaks_hz": tracks.tolist(), "power": powers.tolist(),
              "power_confidence": confidences.tolist(), "loss_db_per_s": losses.tolist(),
              "loss_confidence": loss_confidence.tolist(),
              "early_stereo_rms": float(np.sqrt(np.mean(rms_slice ** 2)))}
    return Measurement(powers, confidences, upper_bounds, location_confidence,
                       losses, loss_confidence, times, sizes, rate, report)


def velocity_weights(velocities):
    velocities = np.asarray(velocities, dtype=float).reshape(-1)
    result = np.zeros((len(velocities), 3))
    for i, velocity in enumerate(velocities):
        if velocity <= KNOTS[0]:
            result[i, 0] = 1.0
        elif velocity >= KNOTS[-1]:
            result[i, -1] = 1.0
        else:
            lo = 0 if velocity < KNOTS[1] else 1
            fraction = (velocity - KNOTS[lo]) / (KNOTS[lo + 1] - KNOTS[lo])
            result[i, lo:lo + 2] = (1 - fraction, fraction)
    return result


def fit_velocity_gains(velocities, corrections, confidence):
    design = velocity_weights(velocities)
    confidence = np.asarray(confidence)
    if not np.any(confidence > 0):
        return np.zeros(3)
    # Small zero prior stabilizes weak observations and unsupported endpoint knots.
    support = design.T @ confidence
    ridge = 0.02 + 0.5 * np.maximum(0, 1 - support)
    matrix = design.T @ (confidence[:, None] * design) + np.diag(ridge)
    # No cross-knot prior: a loud layer must not fill an absent soft-layer partial.
    return np.linalg.solve(matrix, design.T @ (confidence * corrections))


def window_decay_bias(loss, measurement, ti):
    """Finite-Hann-window dB bias relative to power at the exact window center."""
    size = measurement.window_samples[ti]
    t = (np.arange(size) - (size - 1) / 2) / measurement.rate
    w2 = np.hanning(size) ** 2
    exponent = -math.log(10) / 10 * loss * t
    shift = float(np.max(exponent))
    return 10 / math.log(10) * (shift + math.log(float(np.sum(w2 * np.exp(exponent - shift)) / np.sum(w2))))


def validate_measurement_windows(measurements):
    baseline = measurements[0]
    for measurement in measurements:
        times, sizes = np.asarray(measurement.times), np.asarray(measurement.window_samples)
        if (times.ndim != 1 or len(times) < 2 or sizes.shape != times.shape
                or not np.isfinite(times).all() or np.any(np.diff(times) <= 0)
                or not np.isfinite(sizes).all() or np.any(sizes < 4) or np.any(sizes != np.rint(sizes))
                or measurement.rate <= 0
                or any(np.shape(array) != (len(times), PARTIALS) for array in
                       (measurement.power, measurement.confidence, measurement.power_upper_bound))):
            raise ValueError("measurements have invalid time/window shapes")
        if not (np.array_equal(times, baseline.times)
                and np.array_equal(sizes / measurement.rate, baseline.window_samples / baseline.rate)):
            raise ValueError("measurements must use the same time windows and durations")


def fit_note(pairs, reference_offset_db, previous_gain=None, previous_decay=None):
    """Fit residual strike dB and one shared empirical decay ratio per partial."""
    for _, model, reference in pairs:
        validate_measurement_windows((model, reference))
    previous_gain = np.zeros((3, PARTIALS)) if previous_gain is None else previous_gain
    previous_decay = np.ones(PARTIALS) if previous_decay is None else previous_decay
    log_updates, decay_confidence = np.zeros(PARTIALS), np.zeros(PARTIALS)
    for p in range(PARTIALS):
        ratios, weights = [], []
        for _, model, reference in pairs:
            weight = model.loss_confidence[p] * reference.loss_confidence[p]
            if weight > 0.05 and model.loss[p] > 0 and reference.loss[p] > 0:
                ratios.append(math.log(reference.loss[p] / model.loss[p]))
                weights.append(weight)
        if weights:
            center = robust_average(ratios, weights)
            dispersion = weighted_median(abs(np.asarray(ratios) - center), weights)
            quality = min(1.0, sum(weights)) / (1 + (dispersion / 0.35) ** 2)
            log_updates[p] = center * quality
            decay_confidence[p] = quality
    # Light smoothing only on already supported, low-confidence decay estimates.
    regularized = log_updates.copy()
    for p in range(1, PARTIALS - 1):
        if 0 < decay_confidence[p] < 0.8:
            neighbors = decay_confidence[p - 1:p + 2:2]
            if np.sum(neighbors) > 0:
                amount = 0.15 * (1 - decay_confidence[p])
                regularized[p] = (1 - amount) * log_updates[p] + amount * np.average(log_updates[p - 1:p + 2:2], weights=neighbors)
    decay_step = np.clip(np.exp(regularized), *DECAY_STEP)
    decay = np.clip(previous_decay * decay_step, *DECAY_LIMITS)
    applied_ratio = decay / previous_decay
    gain = np.zeros((3, PARTIALS))
    partial_report = []
    for p in range(PARTIALS):
        velocities, corrections, weights = [], [], []
        cut_corrections, cut_weights = [], []
        for velocity, model, reference in pairs:
            values, quality = [], []
            cut_values, cut_quality = [], []
            ratio = applied_ratio[p]
            can_compensate = model.loss_confidence[p] > 0.05
            for ti in (0, 1):
                weight = model.confidence[ti, p] * reference.confidence[ti, p]
                if abs(ratio - 1) > 1e-6 and not can_compensate:
                    continue
                delta = 0.0
                if can_compensate:
                    delta = (-model.loss[p] * (ratio - 1) * model.times[ti]
                             + window_decay_bias(model.loss[p] * ratio, model, ti)
                             - window_decay_bias(model.loss[p], model, ti))
                if reference.confidence[ti, p] <= 0.2:
                    # Censored reference energy can only support attenuation of
                    # a measured model line at a reliably located reference band.
                    upper = reference.power_upper_bound[ti, p]
                    if model.confidence[ti, p] > 0.25 and reference.location_confidence[p] >= 0.25 and upper > 0:
                        ceiling = 10 * math.log10(upper / model.power[ti, p]) + reference_offset_db
                        if ceiling < 0 and ceiling - delta < 0:
                            cut_values.append(ceiling - delta)
                            cut_quality.append(0.25 * model.confidence[ti, p] * reference.location_confidence[p])
                elif weight > 0:
                    correction = 10 * math.log10(reference.power[ti, p] / model.power[ti, p]) + reference_offset_db - delta
                    values.append(correction)
                    quality.append(weight)
            velocities.append(velocity)
            if quality:
                corrections.append(robust_average(values, quality))
                weights.append(float(np.mean(quality)))
            else:
                corrections.append(0.0)
                weights.append(0.0)
            # Choose the least restrictive eligible early-window bound.
            # These are never decay evidence.
            cut_corrections.append(max(cut_values) if cut_values else 0.0)
            cut_weights.append(float(np.mean(cut_quality)) if cut_quality else 0.0)
        velocity_fit = fit_velocity_gains(velocities + velocities,
                                          np.asarray(corrections + cut_corrections), weights + cut_weights)
        # An intermediate layer can constrain two knots algebraically. Do not
        # let it boost spectral noise at a knot whose own render lacks the mode.
        knot_evidence = np.array([weights[velocities.index(v)] if v in velocities else 0.0 for v in KNOTS])
        boost_guard = np.clip(knot_evidence / 0.25, 0, 1)
        raw_update = np.where(velocity_fit > 0, velocity_fit * boost_guard, velocity_fit)
        # Every knot touching a censored observation must remain cut-only. This
        # also guards intermediate velocities against boosts from other layers.
        design = velocity_weights(velocities)
        cut_knots = design.T @ np.asarray(cut_weights) > 0
        raw_update[cut_knots] = np.minimum(raw_update[cut_knots], 0.0)
        update = np.clip(raw_update, -GAIN_STEP, GAIN_STEP)
        gain[:, p] = np.clip(previous_gain[:, p] + update, *GAIN_LIMITS)
        applied_by_velocity = design @ (gain[:, p] - previous_gain[:, p])
        partial_report.append({
            "partial": p + 1, "velocities": velocities,
            "residual_db_by_velocity": corrections, "gain_confidence_by_velocity": weights,
            "cut_only_upper_db_by_velocity": cut_corrections,
            "cut_only_confidence_by_velocity": cut_weights,
            "cut_only_residual_db_before": np.maximum(0, -np.asarray(cut_corrections)).tolist(),
            "cut_only_residual_db_after": np.where(np.asarray(cut_weights) > 0,
                np.maximum(0, applied_by_velocity - cut_corrections), 0).tolist(),
            "knot_cut_only": cut_knots.tolist(),
            "velocity_fit_update_db": velocity_fit.tolist(),
            "knot_boost_evidence_factors": boost_guard.tolist(),
            "unclamped_gain_update_db": raw_update.tolist(),
            "applied_gain_update_db": (gain[:, p] - previous_gain[:, p]).tolist(),
            "gain_clamped": bool(np.any(abs(raw_update - (gain[:, p] - previous_gain[:, p])) > 1e-8)),
            "decay_confidence": float(decay_confidence[p]),
            "unclamped_decay_ratio": float(np.exp(regularized[p])),
            "applied_decay_ratio": float(applied_ratio[p]),
            "decay_clamped": bool(abs(regularized[p] - math.log(applied_ratio[p])) > 1e-8),
        })
    return gain, decay, partial_report


def response_lstsq(design, target, weight, prior):
    """Small ridge fit with 3 dB Huber residual weights (no acoustic model)."""
    effective = weight.copy()
    update = np.zeros(design.shape[1])
    for _ in range(12):
        update = np.linalg.solve(design.T @ (effective[:, None] * design) + np.diag(prior),
                                 design.T @ (effective * target))
        effective = weight * np.minimum(1, 3 / np.maximum(abs(target - design @ update), 1e-12))
    return update, effective


def response_condition(derivative, weight, times):
    """Center within each velocity, so layer differences cannot identify decay."""
    centered = np.zeros_like(derivative)
    eligible = np.zeros_like(weight)
    spans = []
    for i, row in enumerate(weight):
        good = row > 0.05
        span = float(np.ptp(times[i, good])) if np.any(good) else 0.0
        if np.count_nonzero(good) >= 3 and span >= 0.7:
            eligible[i, good] = row[good]
            centered[i] = derivative[i] - np.average(derivative[i, good], weights=row[good])
            spans.append(span)
    support = float(eligible.sum())
    rms = math.sqrt(float(np.sum(eligible * centered ** 2)) / support) if support else 0.0
    total = math.sqrt(float(np.sum(eligible * derivative ** 2)) / support) if support else 0.0
    fraction = rms / max(total, 1e-12)
    identified = rms >= 1.0 and fraction >= 0.05
    confidence = min(1, support / 3) * rms ** 2 / (1 + rms ** 2) * min(1, fraction / 0.1) if identified else 0.0
    return eligible, {"identified": identified, "observations": int(np.count_nonzero(eligible)),
                      "time_span_seconds": max(spans, default=0.0),
                      "centered_derivative_rms_db_per_log_scale": rms,
                      "centered_derivative_fraction": fraction,
                      "conditioning_ratio": total / rms if rms > 1e-12 else None,
                      "confidence": confidence}


def fit_probe_note(triples, reference_offset_db, previous_gain, previous_decay, probe_decay):
    """Fit (velocity, model, probe, reference) Measurements using a local response.

    Loss estimates are deliberately unused: stable beating/rises can carry an
    informative derivative. Floors may constrain gain, never identify decay.
    """
    velocities = [v for v, _, _, _ in triples]
    for _, model, probe, reference in triples:
        validate_measurement_windows((model, probe, reference))
    if len({len(model.times) for _, model, _, _ in triples}) != 1:
        raise ValueError("probe measurements have inconsistent time/window shapes")
    velocity_design = velocity_weights(velocities)
    times = np.array([m.times for _, m, _, _ in triples])
    design = np.repeat(velocity_design, times.shape[1], axis=0)
    gain, decay = np.array(previous_gain, dtype=float), np.array(previous_decay, dtype=float)
    reports = []
    for p in range(PARTIALS):
        derivative, target, tonal_weight = (np.zeros_like(times) for _ in range(3))
        response_weight = np.zeros_like(times)
        log_probe = math.log(probe_decay[p] / previous_decay[p])
        for i, (_, model, probe, reference) in enumerate(triples):
            valid = (model.power[:, p] > 0) & (probe.power[:, p] > 0)
            valid &= (model.confidence[:, p] > 0.2) & (probe.confidence[:, p] > 0.2)
            if abs(log_probe) >= 1e-4:
                derivative[i, valid] = 10 * (np.log10(probe.power[valid, p])
                                            - np.log10(model.power[valid, p])) / log_probe
                response_weight[i, valid] = model.confidence[valid, p] * probe.confidence[valid, p]
            tonal = (reference.confidence[:, p] > 0.2) & (model.confidence[:, p] > 0)
            tonal &= (model.power[:, p] > 0) & (reference.power[:, p] > 0)
            target[i, tonal] = 10 * (np.log10(reference.power[tonal, p])
                                    - np.log10(model.power[tonal, p])) + reference_offset_db
            tonal_weight[i, tonal] = model.confidence[tonal, p] * reference.confidence[tonal, p]
        triple_weight = response_weight * np.array([r.confidence[:, p] for _, _, _, r in triples])
        triple_weight *= tonal_weight > 0
        eligible, condition = response_condition(derivative, triple_weight, times)
        joint = np.zeros(4)
        if condition["identified"]:
            joint_design = np.column_stack((design, derivative.ravel()))
            support = design.T @ eligible.ravel()
            prior = np.r_[0.02 + 0.5 * np.maximum(0, 1 - support), 1.0]
            joint, robust_weight = response_lstsq(joint_design, target.ravel(), eligible.ravel(), prior)
            _, condition = response_condition(derivative, robust_weight.reshape(times.shape), times)
        requested_log = float(joint[3]) if condition["identified"] else 0.0
        bounded_log = float(np.clip(requested_log, *np.log(PROBE_DECAY_STEP)))
        decay[p] = np.clip(previous_decay[p] * math.exp(bounded_log), *DECAY_LIMITS)
        applied_log = math.log(decay[p] / previous_decay[p])
        compensation = derivative * applied_log
        # With a decay update, an unmeasured response cannot be compensated.
        gain_weight = tonal_weight.copy()
        if abs(applied_log) > 1e-12:
            gain_weight *= response_weight > 0
        corrected = target - compensation
        cut_values, cut_weights = [], []
        for i, (_, model, _, reference) in enumerate(triples):
            values, weights = [], []
            for ti in (0, 1):
                if abs(applied_log) > 1e-12 and response_weight[i, ti] <= 0:
                    continue
                upper = reference.power_upper_bound[ti, p]
                if (reference.confidence[ti, p] <= 0.2 and model.confidence[ti, p] > 0.25
                        and reference.location_confidence[p] >= 0.25 and upper > 0 and model.power[ti, p] > 0):
                    ceiling = 10 * math.log10(upper / model.power[ti, p]) + reference_offset_db
                    if ceiling < 0 and ceiling - compensation[i, ti] < 0:
                        values.append(ceiling - compensation[i, ti])
                        weights.append(0.25 * model.confidence[ti, p] * reference.location_confidence[p])
            cut_values.append(max(values) if values else 0.0)
            cut_weights.append(float(np.mean(weights)) if weights else 0.0)
        cut_values, cut_weights = np.array(cut_values), np.array(cut_weights)
        gain_design = np.vstack((design, velocity_design))
        gain_target = np.r_[corrected.ravel(), cut_values]
        weights = np.r_[gain_weight.ravel(), cut_weights]
        support = gain_design.T @ weights
        velocity_fit, _ = response_lstsq(gain_design, gain_target, weights,
                                         0.02 + 0.5 * np.maximum(0, 1 - support))
        by_velocity = gain_weight.mean(axis=1)
        boost_guard = np.clip([by_velocity[velocities.index(v)] / 0.25 if v in velocities else 0 for v in KNOTS], 0, 1)
        raw_update = np.where(velocity_fit > 0, velocity_fit * boost_guard, velocity_fit)
        cut_knots = velocity_design.T @ cut_weights > 0
        raw_update[cut_knots] = np.minimum(raw_update[cut_knots], 0)
        gain[:, p] = np.clip(previous_gain[:, p] + np.clip(raw_update, -GAIN_STEP, GAIN_STEP), *GAIN_LIMITS)
        applied_gain = gain[:, p] - previous_gain[:, p]
        predicted = (design @ applied_gain).reshape(times.shape) + compensation
        residual = target - predicted
        usable = gain_weight > 0
        rms = lambda values: math.sqrt(float(np.sum(gain_weight * values ** 2) / gain_weight.sum())) if np.any(usable) else None
        reports.append({
            "partial": p + 1, "velocities": velocities,
            "gain_confidence_by_velocity": by_velocity.tolist(),
            "residual_db_by_velocity": [float(np.average(row, weights=w)) if np.any(w) else 0.0
                                        for row, w in zip(corrected, gain_weight)],
            "cut_only_upper_db_by_velocity": cut_values.tolist(),
            "cut_only_confidence_by_velocity": cut_weights.tolist(),
            "cut_only_residual_db_before": np.maximum(0, -cut_values).tolist(),
            "cut_only_residual_db_after": np.where(cut_weights > 0,
                np.maximum(0, velocity_design @ applied_gain - cut_values), 0).tolist(),
            "knot_cut_only": cut_knots.tolist(), "knot_boost_evidence_factors": boost_guard.tolist(),
            "velocity_fit_update_db": velocity_fit.tolist(),
            "unclamped_gain_update_db": raw_update.tolist(), "applied_gain_update_db": applied_gain.tolist(),
            "gain_clamped": bool(np.any(abs(raw_update - applied_gain) > 1e-8)),
            "decay_confidence": condition["confidence"],
            "requested_log_decay_update": requested_log, "applied_log_decay_update": applied_log,
            "unclamped_decay_ratio": math.exp(requested_log) if requested_log <= math.log(sys.float_info.max) else None,
            "applied_decay_ratio": float(decay[p] / previous_decay[p]),
            "decay_clamped": abs(requested_log - applied_log) > 1e-8,
            "probe_log_scale_delta": log_probe, "joint_gain_update_db": joint[:3].tolist(),
            "response_condition": condition,
            "derivative_observations": int(np.count_nonzero(response_weight)),
            "derivative_db_per_log_scale": [[float(d) if w > 0 else None for d, w in zip(row, weights)]
                                            for row, weights in zip(derivative, response_weight)],
            "derivative_confidence": response_weight.tolist(),
            "decay_observation_confidence": triple_weight.tolist(),
            "predicted_residual_db": [[float(r) if ok else None for r, ok in zip(row, good)]
                                      for row, good in zip(residual, usable)],
            "predicted_residual_rms_db_before": rms(target), "predicted_residual_rms_db_after": rms(residual),
        })
    return gain, decay, reports


@dataclass(frozen=True)
class Region:
    key: int
    lovel: int
    hivel: int
    sample: Path
    layer: int


def sfz_key(value):
    if re.fullmatch(r"-?\d+", value):
        return int(value)
    match = re.fullmatch(r"([A-Ga-g])([#b]?)(-?\d+)", value)
    if not match:
        raise ValueError(f"invalid SFZ key {value!r}")
    letter, accidental, octave = match.groups()
    return (int(octave) + 1) * 12 + {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}[letter.upper()] + {"": 0, "#": 1, "b": -1}[accidental]


def parse_sfz(corpus):
    corpus = Path(corpus).resolve()
    sfz = corpus / "SalamanderGrandPianoV3.sfz"
    source = sfz.read_text(encoding="utf-8-sig")
    source = re.sub(r"/\*.*?\*/|//[^\n]*", "", source, flags=re.S)
    if re.search(r"^\s*#", source, re.M):
        raise ValueError(f"{sfz}: SFZ preprocessor directives unsupported")
    global_values, group, regions = {}, {}, []
    for match in re.finditer(r"<([^>]+)>([^<]*)", source):
        section, body = match.groups()
        values = {m.group(1): m.group(2).strip().strip('"') for m in re.finditer(r"(\w+)\s*=\s*(.*?)(?=\s+\w+\s*=|$)", body, re.S)}
        if section == "global":
            global_values = values
            group = {}
        elif section == "group":
            group = values
        elif section == "region":
            values = {**global_values, **group, **values}
            name = values.get("sample", "").replace("\\", "/")
            native = re.fullmatch(r"([A-Ga-g][#b]?-?\d+)v(\d+)\.wav", Path(name).name, re.I)
            if not native or values.get("trigger", "attack") not in ("attack", "first"):
                continue
            key = sfz_key(values.get("pitch_keycenter", values.get("key", "60")))
            low = sfz_key(values.get("lokey", values.get("key", "0")))
            high = sfz_key(values.get("hikey", values.get("key", "127")))
            if key != sfz_key(native[1]) or not low <= key <= high:
                raise ValueError(f"{sfz}: non-native keycenter for {name}")
            if float(values.get("tune", 0)) or float(values.get("transpose", 0)):
                raise ValueError(f"{sfz}: retuned region unsupported: {name}")
            sample = (sfz.parent / name).resolve()
            if not sample.is_relative_to(corpus):
                raise ValueError(f"{sfz}: sample escapes corpus: {name}")
            region = Region(key, int(values.get("lovel", 1)), int(values.get("hivel", 127)), sample, int(native[2]))
            if not (0 <= key <= 127 and 1 <= region.lovel <= region.hivel <= 127 and 1 <= region.layer <= 16):
                raise ValueError(f"{sfz}: invalid region: {name}")
            regions.append(region)
    if not regions:
        raise ValueError(f"{sfz}: no native attack regions")
    return sfz, regions


def select_region(regions, key, velocity):
    matches = [r for r in regions if r.key == key and r.lovel <= velocity <= r.hivel]
    if len(matches) != 1:
        raise ValueError(f"native key {key}, velocity {velocity}: expected one SFZ region, found {len(matches)}")
    return matches[0]


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_previous(path):
    return read_calibration(Path(path).read_text(), str(path))


def read_calibration(text, label):
    notes, seen = {}, set()
    with io.StringIO(text, newline="") as source:
        reader = csv.DictReader(source)
        if reader.fieldnames != list(CSV_FIELDS):
            raise ValueError(f"{label}: expected CSV columns {','.join(CSV_FIELDS)}")
        for row in reader:
            if None in row or any(value is None for value in row.values()):
                raise ValueError(f"{label}: expected six CSV fields per row")
            key, partial = int(row["key"]), int(row["partial"])
            values = np.array([float(row[field]) for field in CSV_FIELDS[2:]])
            if not (0 <= key <= 127 and 1 <= partial <= PARTIALS) or (key, partial) in seen:
                raise ValueError(f"{label}: invalid/duplicate key or partial")
            if not np.isfinite(values).all() or np.any(values[:3] < GAIN_LIMITS[0]) or np.any(values[:3] > GAIN_LIMITS[1]) or not DECAY_LIMITS[0] <= values[3] <= DECAY_LIMITS[1]:
                raise ValueError(f"{label}: non-finite/out-of-range calibration")
            seen.add((key, partial))
            gains, decay = notes.setdefault(key, (np.zeros((3, PARTIALS)), np.ones(PARTIALS)))
            gains[:, partial - 1], decay[partial - 1] = values[:3], values[3]
    if not notes or any(sum(k == key for k, _ in seen) != PARTIALS for key in notes):
        raise ValueError(f"{label}: each key must contain exactly {PARTIALS} partials (legacy 64-row tables are unsupported)")
    return notes


def validate_probe(renders, baseline, probe_dir, probe, previous, factor):
    """Require matching renderer settings and complete embedded calibration tables."""
    if not math.isfinite(factor) or factor <= 0 or factor == 1:
        raise ValueError("--probe-decay-factor must be finite, positive and != 1")
    settings, tables = [], []
    required = {"schema", "mode", "rate_hz", "seconds", "notes", "velocities", "dry", "effects",
                "note_on_sample", "note_off", "block_frames", "design_defaults_plus_overrides"}
    for label, root, info, multiplier in (("baseline", renders, baseline, 1.0),
                                          ("probe", probe_dir, probe, factor)):
        if not isinstance(info, dict) or not required <= info.keys():
            raise ValueError(f"{label}: incomplete renderer settings for decay probe")
        json.dumps(info, allow_nan=False)
        if (info["schema"] != 1 or info["mode"] != "calibration" or info["rate_hz"] != 48000
                or not isinstance(info["seconds"], (int, float)) or info["seconds"] < 4
                or type(info["dry"]) is not bool or not isinstance(info["effects"], str)
                or not isinstance(info["design_defaults_plus_overrides"], dict)
                or info["note_on_sample"] != 0 or info["note_off"] is not None):
            raise ValueError(f"{label}: expected held 48 kHz calibration renders with explicit effects")
        calibration = info.get("calibration")
        if not isinstance(calibration, dict) or not isinstance(calibration.get("csv"), str):
            raise ValueError(f"{label}: render.json must embed calibration.csv text")
        table = read_calibration(calibration["csv"], label)
        if table.keys() != previous.keys():
            raise ValueError(f"{label}: calibration keys differ from --previous")
        for key, (gain, decay) in table.items():
            expected_gain, previous_decay = previous[key]
            # Six-place CSV rounding plus f32 parsing/multiplication precision.
            expected_decay = np.clip(previous_decay * multiplier, *DECAY_LIMITS)
            if (not np.allclose(gain, expected_gain, rtol=2e-7, atol=5.1e-7)
                    or not np.allclose(decay, expected_decay, rtol=2e-7, atol=5.1e-7)):
                raise ValueError(f"{label}: calibration does not match expected gains/decay for key {key}")
        comparable = {k: v for k, v in info.items() if k != "calibration"}
        # The source path and CSV necessarily differ; any other voicing metadata must match.
        comparable["calibration"] = {k: v for k, v in calibration.items() if k not in ("path", "csv")}
        for field in ("notes", "velocities"):
            values = info[field]
            if (not isinstance(values, list) or not values
                    or any(type(v) is not int or not 1 <= v <= 127 for v in values)
                    or len(set(values)) != len(values)):
                raise ValueError(f"{label}: invalid manifest {field}")
            comparable[field] = sorted(values)
        expected_files = {f"note_{key:03}_vel_{velocity:03}.wav"
                          for key in info["notes"] for velocity in info["velocities"]}
        actual_files = {p.name for p in root.glob("note_*_vel_*.wav") if p.is_file()}
        if actual_files != expected_files:
            raise ValueError(f"{label}: WAV notes/velocities do not match render.json")
        settings.append(comparable)
        tables.append(table)
    if settings[0] != settings[1]:
        raise ValueError("baseline/probe renderer settings differ (notes, velocities, voicing or effects)")
    return tables[1]


def write_outputs(out, notes, metadata, summary):
    out = Path(out)
    for key, (gain, decay) in notes.items():
        if not 0 <= key <= 127 or gain.shape != (3, PARTIALS) or decay.shape != (PARTIALS,):
            raise ValueError("invalid calibration shape")
        if not np.isfinite(gain).all() or not np.isfinite(decay).all() or np.any(gain < GAIN_LIMITS[0]) or np.any(gain > GAIN_LIMITS[1]) or np.any(decay < DECAY_LIMITS[0]) or np.any(decay > DECAY_LIMITS[1]):
            raise ValueError("invalid calibration values")
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".fit-", dir=out) as staging:
        staging = Path(staging)
        with (staging / OUTPUTS[0]).open("w", newline="") as destination:
            writer = csv.writer(destination, lineterminator="\n")
            writer.writerow(CSV_FIELDS)
            for key, (gain, decay) in sorted(notes.items()):
                for p in range(PARTIALS):
                    writer.writerow([key, p + 1, *(f"{v:.6f}" for v in gain[:, p]), f"{decay[p]:.6f}"])
        rust = ["// Generated empirical modal voicing; closed-loop validation required.",
                "// Reference: Salamander Grand Piano V3, Alexander Holm, CC BY 3.0.",
                "// Source SHA256 manifest and settings: metadata.json.",
                "use super::CalibrationNote;", "", "pub const DEFAULT_CALIBRATION: &[CalibrationNote] = &["]
        for key, (gain, decay) in sorted(notes.items()):
            rust.extend(["    CalibrationNote {", f"        key: {key},", "        gain_db: ["])
            rust.extend("            [" + ", ".join(f"{v:.6f}" for v in row) + "]," for row in gain)
            rust.extend(["        ],", "        decay_scale: [" + ", ".join(f"{v:.6f}" for v in decay) + "],", "    },"])
        rust.append("];\n")
        (staging / OUTPUTS[1]).write_text("\n".join(rust))
        metadata = {**metadata, "generated_sha256": {name: sha256(staging / name) for name in OUTPUTS[:2]}}
        for name, value in ((OUTPUTS[2], metadata), (OUTPUTS[3], summary)):
            (staging / name).write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n")
        for name in OUTPUTS:
            os.replace(staging / name, out / name)


def midi_list(value):
    try:
        values = sorted(set(int(v) for v in value.split(",")))
    except ValueError as error:
        raise argparse.ArgumentTypeError("use comma-separated MIDI integers") from error
    if not values or min(values) < 1 or max(values) > 127:
        raise argparse.ArgumentTypeError("MIDI values must be in 1..127")
    return values


def run(args):
    renders, corpus, out = (Path(p).resolve() for p in (args.renders, args.corpus, args.out))
    probe_dir = Path(args.decay_probe).resolve() if args.decay_probe else None
    if (probe_dir is None) != (args.probe_decay_factor is None) or (probe_dir and not args.previous):
        raise ValueError("--decay-probe and --probe-decay-factor require each other and --previous")
    if out.is_relative_to(renders) or out.is_relative_to(corpus) or (probe_dir and out.is_relative_to(probe_dir)):
        raise ValueError("--out must be outside renders, probe and corpus to preserve sources")
    previous_path = Path(args.previous).resolve() if args.previous else None
    if previous_path and previous_path in [(out / name).resolve() for name in OUTPUTS]:
        raise ValueError("--out must not overwrite --previous")
    render_json = renders / "render.json"
    render_info = json.loads(render_json.read_text())
    if not isinstance(render_info, dict):
        raise ValueError("render.json must contain a JSON object")
    sfz, regions = parse_sfz(corpus)
    native_keys = sorted({region.key for region in regions})
    keys = args.notes or native_keys
    if not args.notes and len(native_keys) != 30:
        raise ValueError(f"expected 30 native keys, found {len(native_keys)}; use --notes for a pilot")
    if any(key not in native_keys for key in keys):
        raise ValueError("--notes contains a non-native SFZ keycenter")
    expected = sorted(set(KNOTS) | set(args.velocities))
    inputs = {}
    for key in keys:
        velocities = set(expected)
        for path in renders.glob(f"note_{key:03}_vel_*.wav"):
            match = re.fullmatch(r"note_\d{3}_vel_(\d{3})\.wav", path.name)
            if not match or not 1 <= int(match[1]) <= 127:
                raise ValueError(f"invalid render filename: {path.name}")
            velocities.add(int(match[1]))
        for velocity in sorted(velocities):
            region = select_region(regions, key, velocity)
            inputs[key, velocity] = (renders / f"note_{key:03}_vel_{velocity:03}.wav", region)
    previous = read_previous(previous_path) if previous_path else {}
    if previous and any(key not in previous for key in keys):
        raise ValueError("--previous must contain every fitted key")
    probe_info, probe_table = None, None
    if probe_dir:
        probe_info = json.loads((probe_dir / "render.json").read_text())
        probe_table = validate_probe(renders, render_info, probe_dir, probe_info, previous, args.probe_decay_factor)
    anchor = {"method": "fixed", "offset_db": args.reference_offset_db}
    if previous_path and (args.reference_offset_db is None or (probe_dir and (previous_path.parent / "metadata.json").is_file())):
        previous_metadata = previous_path.parent / "metadata.json"
        prior = json.loads(previous_metadata.read_text())
        if prior.get("generated_sha256", {}).get("calibration.csv") != sha256(previous_path):
            if probe_dir:
                raise ValueError("previous metadata does not identify this CSV; decay probe requires the original anchor provenance")
            raise ValueError("previous metadata does not identify this CSV; supply --reference-offset-db explicitly")
        if probe_dir and args.reference_offset_db is not None and not math.isclose(args.reference_offset_db, prior["anchor"]["offset_db"], abs_tol=1e-9, rel_tol=0):
            raise ValueError("decay probe must reuse the previous global reference offset")
        anchor = {**prior["anchor"], "reused_from_previous": True}
    elif args.reference_offset_db is None:
        region = select_region(regions, 60, 68)
        inputs.setdefault((60, 68), (renders / "note_060_vel_068.wav", region))
        anchor = {"method": "C4_68_stereo_rms_0.05_to_0.45s", "key": 60, "velocity": 68, "offset_db": None}
    if (probe_dir and anchor["offset_db"] is None) or (anchor["offset_db"] is not None and not math.isfinite(anchor["offset_db"])):
        raise ValueError("reference offset must be finite")
    missing = [str(path) for model, region in inputs.values() for path in (model, region.sample) if not path.is_file()]
    if missing:
        raise ValueError("missing required inputs (no pitch/layer substitution):\n" + "\n".join(sorted(set(missing))))
    source_files = {sfz, *(region.sample for _, region in inputs.values())}
    if (corpus / "README").is_file():
        source_files.add(corpus / "README")
    manifest = lambda paths, root: [{"path": str(path.relative_to(root)), "sha256": sha256(path), "bytes": path.stat().st_size} for path in sorted(paths)]
    source_manifest = manifest(source_files, corpus)
    render_manifest = manifest({render_json, *(model for model, _ in inputs.values())}, renders)
    probe_manifest = manifest({probe_dir / "render.json", *(probe_dir / model.name for model, _ in inputs.values())}, probe_dir) if probe_dir else None

    def measure(path, key, model):
        samples, rate, fmt = read_wav(path)
        if rate != 48000 or fmt["channels"] != 2 or fmt["encoding"] != (3 if model else 1) or fmt["bits"] != (32 if model else 24):
            raise ValueError(f"{path}: expected stereo 48 kHz {'float32 render' if model else 'PCM24 reference'}")
        if model and len(samples) < rate * 4 - 1:
            raise ValueError(f"{path}: expected held render of at least 4 seconds")
        return analyze(samples, rate, key)

    cached_anchor = None
    if anchor["offset_db"] is None:
        model_path, region = inputs[60, 68]
        model, reference = measure(model_path, 60, True), measure(region.sample, 60, False)
        anchor["offset_db"] = 20 * math.log10(model.report["early_stereo_rms"] / reference.report["early_stereo_rms"])
        anchor["model_rms"], anchor["reference_rms"] = model.report["early_stereo_rms"], reference.report["early_stereo_rms"]
        anchor["original_model_sha256"] = sha256(model_path)
        anchor["original_reference_sha256"] = sha256(region.sample)
        cached_anchor = (model, reference)
    notes, reports = {}, []
    for key in keys:
        pairs, triples, observations, reference_cache = [], [], [], {}
        for (input_key, velocity), (model_path, region) in sorted(inputs.items()):
            if input_key != key:
                continue
            if (key, velocity) == (60, 68) and cached_anchor is not None:
                model, reference = cached_anchor
            else:
                model = measure(model_path, key, True)
                if region.sample not in reference_cache:
                    reference_cache[region.sample] = measure(region.sample, key, False)
                reference = reference_cache[region.sample]
            pairs.append((velocity, model, reference))
            observations.append({"velocity": velocity, "layer": region.layer,
                                 "reference": str(region.sample.relative_to(corpus)),
                                 "model": model.report, "reference_measurement": reference.report})
            if probe_dir:
                probe = measure(probe_dir / model_path.name, key, True)
                triples.append((velocity, model, probe, reference))
                observations[-1]["probe_measurement"] = probe.report
        if probe_dir:
            gain, decay, partial_report = fit_probe_note(triples, anchor["offset_db"], *previous[key], probe_table[key][1])
        else:
            gain, decay, partial_report = fit_note(pairs, anchor["offset_db"], *previous.get(key, (None, None)))
        notes[key] = gain, decay
        reports.append({"key": key, "inputs": observations, "partials": partial_report})
        print(f"fitted native key {key}: {len(pairs)} velocities", file=sys.stderr)
    metadata = {
        "schema_version": 1, "algorithm": "empirical-stereo-modal-voicing-v1",
        "script_sha256": sha256(__file__), "numpy_version": np.__version__, "python_version": sys.version,
        "reference_attribution": {"title": "Salamander Grand Piano V3", "author": "Alexander Holm", "license": "CC BY 3.0", "license_url": "https://creativecommons.org/licenses/by/3.0/"},
        "source_manifest": source_manifest, "render_manifest": render_manifest,
        "render_metadata": render_info, "anchor": anchor, "keys": keys,
        "expected_velocities": expected, "velocity_knots": KNOTS, "partials": PARTIALS,
        "windows_seconds": TIMES.tolist(), "window_durations_seconds": WINDOW_SECONDS.tolist(),
        "gain_limits_db": GAIN_LIMITS, "gain_step_limit_db": GAIN_STEP,
        "decay_limits": DECAY_LIMITS, "decay_step_limits": DECAY_STEP,
        "measurement_settings": {
            "onset_block_seconds": 0.001, "onset_peak_search_seconds": 0.5,
            "onset_relative_threshold_db": -40, "identification_seconds": [0.1, 1.2],
            "early_identification_seconds": [0, EARLY_SECONDS],
            "early_match_radius": "min(0.23 * harmonic gap, 2.5 * max(2 / early duration, 0.003 * predicted Hz))",
            "f0_search_cents": [-45, 45], "B_search": [0, 0.01],
            "line_prominence_min_db": 15, "band_snr_fade_db": [10, 25],
            "relative_band_power_fade_db": [-60, -40],
            "band_half_width": "min(0.23 * harmonic gap, max(2.5 / window duration, 0.008 * center Hz))",
            "gain_velocity_zero_prior": "0.02 + 0.5 * max(0, 1 - knot support)",
            "positive_gain_knot_evidence_full_at": 0.25,
            "power_upper_bound": "predicted harmonic band total power plus one local-floor allowance",
            "cut_only_weight_scale": 0.25,
            "cut_only_location_confidence_min": 0.25,
            "cut_only_model_confidence_min": 0.25,
            "cut_only_reference_confidence_max": 0.2,
            "decay_neighborhood_strength_max": 0.15,
        },
        "previous_sha256": sha256(previous_path) if previous_path else None,
        "reference_target": "native recorded PCM; SFZ used for key/layer selection, no SFZ gain or retuning",
        "acceptance": "unvalidated; requires held-out and closed-loop rerender measurement",
    }
    if probe_dir:
        metadata.update({"algorithm": "empirical-stereo-modal-voicing-probe-v1",
                         "probe_manifest": probe_manifest, "probe_metadata": probe_info,
                         "probe_decay_factor": args.probe_decay_factor, "decay_step_limits": PROBE_DECAY_STEP})
        metadata["measurement_settings"].update({
            "response": "(probe_power_db - model_power_db) / log(actual_probe_scale / previous_scale)",
            "probe_log_scale_delta_min": 1e-4, "response_tonal_confidence_min": 0.2,
            "response_time_support_weight_min": 0.05,
            "response_min_windows_per_velocity": 3, "response_min_time_span_seconds": 0.7,
            "response_centered_rms_min_db_per_log_scale": 1.0, "response_centered_fraction_min": 0.05,
            "response_huber_db": 3.0, "response_irls_iterations": 12, "response_log_decay_zero_prior": 1.0,
            "decay_neighborhood_strength_max": 0.0,
        })
    all_partials = [p for report in reports for p in report["partials"]]
    summary = {"acceptance": metadata["acceptance"], "anchor": anchor,
               "fitted_keys": len(notes), "fitted_inputs": sum(len(r["inputs"]) for r in reports),
               "gain_clamped_partials": sum(p["gain_clamped"] for p in all_partials),
               "decay_clamped_partials": sum(p["decay_clamped"] for p in all_partials),
               "unsupported_partials": sum(not any(p["gain_confidence_by_velocity"] + p["cut_only_confidence_by_velocity"]) for p in all_partials),
               "notes": reports}
    write_outputs(out, notes, metadata, summary)
    print(f"Wrote {len(notes)} keys / {len(notes) * PARTIALS} rows to {out}; closed-loop validation required.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renders", required=True, type=Path)
    parser.add_argument("--corpus", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path, help="explicit output directory; only four named outputs may be replaced")
    parser.add_argument("--previous", type=Path)
    parser.add_argument("--decay-probe", type=Path, help="matched renders with previous decay scales perturbed; requires --previous and --probe-decay-factor")
    parser.add_argument("--probe-decay-factor", type=float, help="positive non-unit multiplier used for every probe decay scale before clamping (typically 0.7)")
    parser.add_argument("--velocities", type=midi_list, default=list(KNOTS), help="comma-separated required velocities; knots are always required; available extra layers also contribute")
    parser.add_argument("--notes", type=midi_list, help="comma-separated native MIDI keys for a pilot; default all 30")
    parser.add_argument("--reference-offset-db", type=float, help="fixed shared dB offset added to reference; otherwise anchor C4/68, or reuse previous metadata")
    args = parser.parse_args(argv)
    try:
        run(args)
    except (OSError, ValueError, KeyError, struct.error) as error:
        parser.exit(2, f"fit_voicing: {error}\n")


if __name__ == "__main__":
    main()
