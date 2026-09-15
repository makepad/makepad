"""Synthetic-only numerical and CLI contract tests; no devices or real corpus."""

import contextlib
import copy
import csv
import io
import json
from pathlib import Path
import re
import struct
import tempfile
import unittest
from unittest import mock

import numpy as np

import fit_voicing as fit


CHECKOUT = Path(__file__).resolve().parents[3]


def write_wav(path, samples, rate, encoding=3, bits=32, extensible=False):
    samples = np.asarray(samples)
    channels = samples.shape[1]
    align = channels * bits // 8
    if encoding == 3:
        payload = samples.astype("<f4").tobytes()
    elif bits == 24:
        integers = np.clip(np.rint(samples * 8388608), -8388608, 8388607).astype(np.int32).ravel()
        payload = np.column_stack([integers & 255, (integers >> 8) & 255, (integers >> 16) & 255]).astype(np.uint8).tobytes()
    else:
        payload = (samples * 2 ** (bits - 1)).astype(f"<i{bits // 8}").tobytes()
    fmt = struct.pack("<HHIIHH", 0xFFFE if extensible else encoding, channels, rate, rate * align, align, bits)
    if extensible:
        fmt += struct.pack("<HHII", 22, bits, 3, encoding) + bytes.fromhex("00001000800000aa00389b71")
    chunks = b"JUNK" + struct.pack("<I", 3) + b"abc\0"
    chunks += b"fmt " + struct.pack("<I", len(fmt)) + fmt
    chunks += b"data" + struct.pack("<I", len(payload)) + payload + (b"\0" if len(payload) & 1 else b"")
    Path(path).write_bytes(b"RIFF" + struct.pack("<I", len(chunks) + 4) + b"WAVE" + chunks)


def note(key=57, rate=8000, duration=2.0, f0=None, stiffness=0.0006,
         gain_db=0.0, decay_ratio=1.0, absent=(), noise=0.0, unison=False, partial_db=None):
    time = np.arange(round(duration * rate)) / rate
    f0 = fit.midi_frequency(key) if f0 is None else f0
    frequencies = fit.partial_frequencies(f0, stiffness, 10)
    result = np.zeros((len(time), 2))
    for p, frequency in enumerate(frequencies):
        if p + 1 in absent or frequency >= rate * 0.46:
            continue
        level = gain_db + (partial_db or {}).get(p + 1, 0.0)
        envelope = 0.055 / (p + 1) ** 0.85 * 10 ** (level / 20) * np.exp(-(0.7 + p * 0.06) * decay_ratio * time)
        tone = envelope * np.sin(2 * np.pi * frequency * time + p * 0.31)
        result[:, 0] += tone
        result[:, 1] -= 0.8 * tone  # Anti-phase stereo must not collapse.
        if unison:
            result[:, 0] += envelope * 0.35 * np.sin(2 * np.pi * (frequency * 1.002) * time + 1)
            result[:, 1] += envelope * 0.35 * np.cos(2 * np.pi * (frequency * 0.998) * time)
    if noise:
        result += np.random.default_rng(42).normal(0, noise, result.shape)
    return np.concatenate([np.zeros((round(0.04 * rate), 2)), result])


def attack_note(partials=(64, 80, 100), gain_db=0, f0=27.5, stiffness=0.0006, noise=0.0005):
    """Stable A0 low modes plus high tones that vanish before the long FFT."""
    samples = note(key=21, rate=48000, f0=f0, stiffness=stiffness)
    time = np.arange(len(samples)) / 48000 - 0.04
    envelope = np.exp(-40 * np.maximum(time, 0)) * (time >= 0) * np.clip((0.09 - time) / 0.02, 0, 1)
    for partial in partials:
        frequency = f0 * partial * np.sqrt(1 + stiffness * partial ** 2)
        tone = 0.04 * 10 ** (gain_db / 20) * envelope * np.sin(2 * np.pi * frequency * time)
        samples += tone[:, None] * np.array([1, -0.8])
    samples += np.random.default_rng(39).normal(0, noise, samples.shape) * envelope[:, None]
    return samples


class WavAndPowerTests(unittest.TestCase):
    def test_pcm24_float_and_extensible_reading(self):
        samples = np.array([[-1, 0.99999988], [-0.5, 0.5], [-1 / 8388608, 1 / 8388608], [0, 0]])
        with tempfile.TemporaryDirectory(dir=CHECKOUT) as directory:
            for encoding, bits in ((1, 24), (3, 32), (1, 16), (1, 32)):
                for extensible in (False, True):
                    path = Path(directory) / "sample.wav"
                    write_wav(path, samples, 48000, encoding, bits, extensible)
                    actual, rate, fmt = fit.read_wav(path)
                    np.testing.assert_allclose(actual, samples, atol=2 ** (1 - bits))
                    self.assertEqual(rate, 48000)
                    self.assertEqual(fmt, {"channels": 2, "encoding": encoding, "bits": bits})

    def test_truncated_and_nonfinite_wav_rejected(self):
        with tempfile.TemporaryDirectory(dir=CHECKOUT) as directory:
            path = Path(directory) / "bad.wav"
            write_wav(path, np.zeros((20, 2)), 48000)
            path.write_bytes(path.read_bytes()[:-3])
            with self.assertRaisesRegex(ValueError, "truncated"):
                fit.read_wav(path)
            write_wav(path, np.full((20, 2), np.nan), 48000)
            with self.assertRaisesRegex(ValueError, "non-finite"):
                fit.read_wav(path)

    def test_antiphase_power_and_fft_normalization(self):
        time = np.arange(2400) / 8000
        sine = 0.25 * np.sin(2 * np.pi * 220 * time)
        antiphase = np.column_stack([sine, -sine])
        inphase = np.column_stack([sine, sine])
        for nfft in (2400, 4096, 8192):
            _, power = fit.power_spectrum(antiphase, 8000, nfft)
            _, other = fit.power_spectrum(inphase, 8000, nfft)
            np.testing.assert_allclose(power, other, atol=1e-16)
            self.assertAlmostEqual(float(np.sum(power)), 0.25 ** 2 / 2, places=8)
        self.assertEqual(float(np.max(abs(np.mean(antiphase, axis=1)))), 0)

    def test_onset_threshold_in_millisecond_blocks(self):
        samples = np.zeros((48000, 2))
        samples[4800:9600, 0] = 0.00005  # Below -40 dB relative to later peak.
        samples[9600:, 0], samples[9600:, 1] = 0.1, -0.1
        self.assertEqual(fit.onset_index(samples, 48000), 9600)
        with self.assertRaisesRegex(ValueError, "silent onset"):
            fit.onset_index(np.zeros_like(samples), 48000)


class SpectralFitTests(unittest.TestCase):
    def test_early_only_high_partials_cut_with_independent_reference(self):
        model = fit.analyze(attack_note(), 48000, 21)
        reference = fit.analyze(attack_note(gain_db=-6, f0=27.5 * 1.003, stiffness=0.0008), 48000, 21)
        gain, decay, report = fit.fit_note([(v, model, reference) for v in fit.KNOTS], 0)
        high = np.array([64, 80, 100]) - 1
        for measured_note in (model, reference):
            pitch = measured_note.report["pitch_fit"]
            np.testing.assert_array_equal(np.array(pitch["long_line_confidence"])[high], 0)
            self.assertTrue(np.all(np.array(pitch["early_line_used"])[high]))
            self.assertTrue(np.all(measured_note.confidence[0, high] > 0.8))
            np.testing.assert_array_equal(measured_note.confidence[1:, high], 0)
            # Early tail evidence cannot fill the missing-reference gaps or extend it.
            np.testing.assert_array_equal(measured_note.location_confidence[[50, 90, 120]], 0)
        np.testing.assert_allclose(gain[:, high], -6, atol=0.25)
        np.testing.assert_array_equal(decay[high], 1)
        self.assertTrue(all(report[p]["gain_confidence_by_velocity"][0] > 0.8 for p in high))
        gain, decay, report = fit.fit_probe_note([(v, model, model, reference) for v in fit.KNOTS], 0,
            np.zeros((3, fit.PARTIALS)), np.ones(fit.PARTIALS), np.full(fit.PARTIALS, 0.7))
        np.testing.assert_allclose(gain[:, high], -6, atol=0.25)
        np.testing.assert_array_equal(decay[high], 1)
        self.assertFalse(report[99]["response_condition"]["identified"])
        # The independently fitted long curve and already-good centers stay fixed.
        stable = fit.analyze(note(key=21, rate=48000), 48000, 21)
        for field in ("f0_hz", "B", "confidence", "supported_lines"):
            self.assertEqual(model.report["pitch_fit"][field], stable.report["pitch_fit"][field])
        np.testing.assert_array_equal(model.report["partial_centers_hz"][:10], stable.report["partial_centers_hz"][:10])

    def test_broadband_attack_cannot_supply_missing_model_high_modes(self):
        model = fit.analyze(attack_note(partials=(), noise=0.03), 48000, 21)
        reference = fit.analyze(attack_note(gain_db=6), 48000, 21)
        gain, decay, _ = fit.fit_note([(v, model, reference) for v in fit.KNOTS], 0)
        np.testing.assert_array_equal(model.confidence[:, 10:], 0)
        np.testing.assert_array_equal(gain[:, 10:], 0)
        np.testing.assert_array_equal(decay[10:], 1)
        gain, decay, _ = fit.fit_probe_note([(v, model, model, reference) for v in fit.KNOTS], 0,
            np.zeros((3, fit.PARTIALS)), np.ones(fit.PARTIALS), np.full(fit.PARTIALS, 0.7))
        np.testing.assert_array_equal(gain[:, 10:], 0)
        np.testing.assert_array_equal(decay[10:], 1)
        # A missing high reference is not located using the model's early line.
        gain, decay, report = fit.fit_note([(v, reference, model) for v in fit.KNOTS], 0)
        np.testing.assert_array_equal(gain[:, 10:], 0)
        self.assertFalse(any(report[99]["cut_only_confidence_by_velocity"]))

    def test_early_peak_proximity_and_unique_assignment(self):
        time = np.arange(7200) / 48000
        predicted, gaps = np.array([1000., 1100.]), np.array([100., 100.])
        for frequency, count in ((1004, 1), (1050, 0)):
            samples = np.sin(2 * np.pi * frequency * time)[:, None]
            centers, confidence = fit.early_partial_lines(samples, 48000, predicted, gaps)
            self.assertEqual(np.count_nonzero(confidence > 0.25), count)
            if count:
                self.assertAlmostEqual(centers[0], frequency, delta=0.2)
                self.assertEqual(confidence[1], 0)

    def test_exact_mixed_windows_and_duration_specific_decay_compensation(self):
        model = fit.analyze(note(), 8000, 57)
        # Independently stated sample windows, including the true 0--120 ms attack.
        np.testing.assert_array_equal(model.window_samples, [960, 2400, 2400, 2400, 2400])
        np.testing.assert_array_equal(model.report["window_start_samples"], [0, 1600, 4000, 6800, 10000])
        np.testing.assert_allclose(model.times, np.array([0.06, 0.35, 0.65, 1, 1.4]) - 0.5 / 8000)
        for ti, size in enumerate(model.window_samples):
            time = np.arange(size) / 8000
            weighted_power = np.average(10 ** (-18 * time / 10), weights=np.hanning(size) ** 2)
            expected_bias = 10 * np.log10(weighted_power) + 18 * (size - 1) / 16000
            self.assertAlmostEqual(fit.window_decay_bias(18, model, ti), expected_bias, places=12)
        self.assertGreater(fit.window_decay_bias(18, model, 1), 5 * fit.window_decay_bias(18, model, 0))

    def test_analytic_gain_decay_recovery_from_mixed_hann_windows(self):
        times = np.array([0.06, 0.35, 0.65, 1, 1.4]) - 0.5 / 48000
        durations = [0.12, 0.30, 0.30, 0.30, 0.30]

        def exponential(loss, gain):
            levels = []
            for center, duration in zip(times, durations):
                size = round(duration * 48000)
                time = center + (np.arange(size) - (size - 1) / 2) / 48000
                levels.append(gain + 10 * np.log10(np.average(10 ** (-loss * time / 10),
                                                              weights=np.hanning(size) ** 2)))
            return measured(levels, times=times, durations=durations)

        model = exponential(30, 0)
        pairs = [(v, model, exponential(45, g)) for v, g in zip(fit.KNOTS, [-3, 2, 6])]
        gain, decay, _ = fit.fit_note(pairs, 0)
        self.assertAlmostEqual(model.loss[0], 30, places=10)
        self.assertAlmostEqual(decay[0], 1.5, places=10)
        np.testing.assert_allclose(gain[:, 0], [-3, 2, 6], atol=0.13)

    def test_known_stiff_string_pitch_independently_fitted(self):
        for key, cents, stiffness in ((33, -8, 0.00012), (57, 7, 0.0008), (81, 15, 0.002)):
            f0 = fit.midi_frequency(key) * 2 ** (cents / 1200)
            samples = note(key=key, rate=48000, f0=f0, stiffness=stiffness)
            result = fit.analyze(samples, 48000, key)
            measured = result.report["pitch_fit"]
            self.assertLess(abs(1200 * np.log2(measured["f0_hz"] / f0)), 1.2)
            self.assertLess(abs(measured["B"] - stiffness), max(stiffness * 0.15, 0.000015))
            self.assertGreater(measured["confidence"], 0.6)

    def test_gain_and_decay_recovered_with_independent_frequencies(self):
        model = fit.analyze(note(), 8000, 57)
        reference = fit.analyze(note(gain_db=6, decay_ratio=1.5, f0=fit.midi_frequency(57) * 1.003,
                                     stiffness=0.0009), 8000, 57)
        gain, decay, _ = fit.fit_note([(v, model, reference) for v in fit.KNOTS], 0)
        np.testing.assert_allclose(model.loss[:8], (0.7 + np.arange(8) * 0.06) * 20 / np.log(10), atol=0.08)
        np.testing.assert_allclose(decay[:8], 1.5, atol=0.035)
        np.testing.assert_allclose(gain[:, :8], 6, atol=0.35)
        self.assertTrue(np.isfinite(gain).all())
        self.assertTrue(np.isfinite(decay).all())

    def test_absent_and_noise_only_partial_not_boosted(self):
        model = fit.analyze(note(absent=(2, 7), noise=0.00003), 8000, 57)
        reference = fit.analyze(note(gain_db=12), 8000, 57)
        gain, decay, report = fit.fit_note([(v, model, reference) for v in fit.KNOTS], 0)
        for partial in (2, 7, 50, fit.PARTIALS):
            np.testing.assert_allclose(gain[:, partial - 1], 0, atol=0.5)
            self.assertAlmostEqual(decay[partial - 1], 1, delta=0.02)
            self.assertLess(max(report[partial - 1]["gain_confidence_by_velocity"]), 0.05)

    def test_missing_or_weak_reference_fundamental_cut_without_decay(self):
        for key in (21, 57):
            model = fit.analyze(note(key=key), 8000, key)
            for absent in ((1,), ()):
                with self.subTest(key=key, absent=absent):
                    reference = fit.analyze(note(key=key, absent=absent, partial_db={1: -65},
                        noise=0.00015, gain_db=4, f0=fit.midi_frequency(key) * 1.012,
                        stiffness=0.0011), 8000, key)
                    np.testing.assert_array_equal(reference.confidence[:, 0], 0)
                    self.assertGreater(reference.location_confidence[0], 0.25)
                    # A0 is unresolved in 120 ms; the unchanged 350 ms window
                    # supplies the conservative fundamental cut in that case.
                    self.assertGreater(model.confidence[1, 0], 0.25)
                    self.assertTrue(np.all(reference.power_upper_bound[1:, 0] > 0))
                    self.assertTrue(np.isfinite(reference.power_upper_bound).all())
                    self.assertGreater(abs(reference.report["predicted_centers_hz"][0]
                                           - model.report["predicted_centers_hz"][0]), 0.25)
                    self.assertEqual(reference.loss[0], 0)
                    self.assertEqual(reference.loss_confidence[0], 0)
                    pairs = [(v, model, reference) for v in fit.REPRESENTATIVE_VELOCITIES]
                    gain, decay, report = fit.fit_note(pairs, -4)
                    self.assertTrue(np.isfinite(gain).all() and np.isfinite(decay).all())
                    self.assertTrue(np.all(gain[:, 0] < -1))
                    self.assertTrue(np.all(gain[:, 0] >= -fit.GAIN_STEP))
                    self.assertEqual(decay[0], 1)
                    self.assertEqual(report[0]["decay_confidence"], 0)
                    before = np.array(report[0]["cut_only_residual_db_before"])
                    after = np.array(report[0]["cut_only_residual_db_after"])
                    self.assertTrue(np.all(after < before))
                    self.assertTrue(np.isfinite(after).all() and np.all(after >= 0))
                    np.testing.assert_allclose(gain[:, 1:6], 0, atol=0.4)

                    # Previous absolute limits still bound a censored iteration.
                    prior = np.full((3, fit.PARTIALS), -32.0)
                    gain, decay, _ = fit.fit_note(pairs, -4, prior, np.full(fit.PARTIALS, 2.7))
                    np.testing.assert_array_equal(gain[:, 0], -36)
                    self.assertEqual(decay[0], 2.7)

                    # Reversing the absence cannot turn a noise-only model into a boost.
                    gain, decay, report = fit.fit_note([(v, reference, model) for v in fit.KNOTS], 0)
                    np.testing.assert_array_equal(gain[:, 0], 0)
                    self.assertEqual(decay[0], 1)
                    self.assertFalse(any(report[0]["cut_only_confidence_by_velocity"]))

    def test_reference_upper_bound_requires_location_and_shared_level_excess(self):
        model = fit.analyze(note(), 8000, 57)
        missing = fit.analyze(note(absent=(1,), noise=0.00015), 8000, 57)
        # Shift the shared target above the model: an upper bound is not a boost target.
        offset = float(np.max(10 * np.log10(model.power[:2, 0] / missing.power_upper_bound[:2, 0]))) + 1
        gain, decay, report = fit.fit_note([(v, model, missing) for v in fit.KNOTS], offset)
        np.testing.assert_array_equal(gain[:, 0], 0)
        self.assertEqual(decay[0], 1)
        self.assertFalse(any(report[0]["cut_only_confidence_by_velocity"]))

        # One surviving harmonic cannot locate a missing fundamental reliably.
        uncertain = fit.analyze(note(absent=(1, *range(3, 11)), noise=0.00015), 8000, 57)
        self.assertEqual(uncertain.location_confidence[0], 0)
        self.assertTrue(np.all(uncertain.power_upper_bound[:2, 0] > 0))
        gain, decay, report = fit.fit_note([(v, model, uncertain) for v in fit.KNOTS], 0)
        np.testing.assert_array_equal(gain[:, 0], 0)
        self.assertEqual(decay[0], 1)
        self.assertFalse(any(report[0]["cut_only_confidence_by_velocity"]))

    def test_other_velocities_cannot_boost_censored_intermediate_layer(self):
        model = fit.analyze(note(), 8000, 57)
        loud = fit.analyze(note(gain_db=12), 8000, 57)
        missing = fit.analyze(note(absent=(1,), noise=0.00015), 8000, 57)
        velocities = fit.REPRESENTATIVE_VELOCITIES
        pairs = [(v, model, missing if v == 48 else loud) for v in velocities]
        gain, decay, report = fit.fit_note(pairs, 0)
        partial = report[0]
        self.assertEqual(partial["knot_cut_only"], [True, True, False])
        self.assertTrue(np.all(np.asarray(partial["velocity_fit_update_db"])[:2] > 0))
        self.assertTrue(np.all(gain[:2, 0] <= 0))
        self.assertLessEqual(float(fit.velocity_weights([48])[0] @ gain[:, 0]), 0)
        self.assertGreater(gain[2, 0], 10)
        self.assertAlmostEqual(decay[0], 1, delta=0.001)
        self.assertLessEqual(partial["cut_only_residual_db_after"][velocities.index(48)],
                             partial["cut_only_residual_db_before"][velocities.index(48)])

    def test_unison_cluster_gain_preserved(self):
        model = fit.analyze(note(unison=True), 8000, 57)
        reference = fit.analyze(note(unison=True, gain_db=4), 8000, 57)
        gain, decay, _ = fit.fit_note([(v, model, reference) for v in fit.KNOTS], 0)
        np.testing.assert_allclose(gain[:, :6], 4, atol=0.35)
        np.testing.assert_allclose(decay[:6], 1, atol=0.01)

    def test_extra_layer_cannot_boost_missing_soft_knot_partial(self):
        model = fit.analyze(note(), 8000, 57)
        soft = fit.analyze(note(absent=(2,), noise=0.00003), 8000, 57)
        reference = fit.analyze(note(gain_db=8), 8000, 57)
        pairs = [(28, soft, reference)] + [(v, model, reference) for v in (48, 68, 112)]
        gain, _, report = fit.fit_note(pairs, 0)
        self.assertEqual(gain[0, 1], 0)
        self.assertGreater(gain[1, 1], 7)
        self.assertEqual(report[1]["knot_boost_evidence_factors"][0], 0)

    def test_rising_and_beating_decay_rejected(self):
        for levels in ([0, 1, 3, 5, 7], [0, -2, -13, -5, -7], [0, -1, -1.1, -1.2, -1.3]):
            _, confidence = fit.robust_decay(fit.TIMES, 10 ** (np.asarray(levels) / 10), np.ones(5))
            self.assertEqual(confidence, 0)

    def test_previous_adds_gain_multiplies_decay_and_clamps(self):
        model = fit.analyze(note(), 8000, 57)
        reference = fit.analyze(note(gain_db=35, decay_ratio=3), 8000, 57)
        previous_gain = np.full((3, fit.PARTIALS), 20.0)
        previous_decay = np.full(fit.PARTIALS, 3.0)
        gain, decay, report = fit.fit_note([(v, model, reference) for v in fit.KNOTS], 0, previous_gain, previous_decay)
        np.testing.assert_allclose(gain[:, :6], 24, atol=0.01)
        np.testing.assert_allclose(decay[:6], 4, atol=0.01)
        self.assertTrue(report[0]["gain_clamped"])
        self.assertTrue(report[0]["decay_clamped"])
        model_only = fit.analyze(note(absent=(2,)), 8000, 57)
        gain, decay, _ = fit.fit_note([(v, model_only, reference) for v in fit.KNOTS], 0, previous_gain, previous_decay)
        np.testing.assert_allclose(gain[:, 1], 20, atol=0.01)
        self.assertAlmostEqual(decay[1], 3)


def measured(levels, confidence=1.0, times=None, durations=None):
    """One tonal partial, independent of any exponential envelope assumption."""
    # Preserve the original independent response benchmark's windows by default.
    times = np.array([0.15, 0.35, 0.65, 1.0, 1.4]) if times is None else np.array(times)
    durations = np.full(len(times), 0.30) if durations is None else np.array(durations)
    power = np.zeros((len(times), fit.PARTIALS))
    power[:, 0] = 10 ** (np.asarray(levels) / 10)
    quality = np.zeros_like(power)
    quality[:, 0] = confidence
    loss, loss_confidence = np.zeros(fit.PARTIALS), np.zeros(fit.PARTIALS)
    loss[0], loss_confidence[0] = fit.robust_decay(times, power[:, 0], quality[:, 0])
    return fit.Measurement(power, quality, power.copy(), np.ones(fit.PARTIALS),
                           loss, loss_confidence, times, np.rint(durations * 48000).astype(int), 48000, {})


class ProbeResponseTests(unittest.TestCase):
    def fixture(self, gains=(-3, 2, 6), ratio=1.4, factor=0.7, derivative=None,
                velocities=(28, 48, 68, 90, 112), offset=-6, previous_scale=1.0,
                times=None, durations=None):
        previous_gain = np.full((3, fit.PARTIALS), 1.5)
        previous_decay = np.full(fit.PARTIALS, previous_scale)
        probe_decay = np.clip(previous_decay * factor, *fit.DECAY_LIMITS)
        triples = []
        for v, gain in zip(velocities, fit.velocity_weights(velocities) @ gains):
            d = np.array([-2, -5, -9, -14, -22]) + (v - 68) * 0.02 if derivative is None else np.asarray(derivative)
            baseline = np.array([-20, -25, -22, -31, -24])
            triples.append((v, measured(baseline, times=times, durations=durations),
                            measured(baseline + d * np.log(probe_decay[0] / previous_scale), times=times, durations=durations),
                            measured(baseline + gain + d * np.log(ratio) - offset, times=times, durations=durations)))
        return triples, offset, previous_gain, previous_decay, probe_decay

    def test_mixed_window_response_gain_decay_recovery(self):
        args = self.fixture(times=[0.06, 0.35, 0.65, 1, 1.4], durations=[0.12, 0.30, 0.30, 0.30, 0.30])
        gain, decay, report = fit.fit_probe_note(*args)
        np.testing.assert_allclose(gain[:, 0] - args[2][:, 0], [-3, 2, 6], atol=0.04)
        self.assertAlmostEqual(decay[0], 1.4, delta=0.005)
        self.assertAlmostEqual(report[0]["response_condition"]["time_span_seconds"], 1.34)
        for i, (_, model, probe, _) in enumerate(args[0]):
            probe.power[:, 0] = model.power[:, 0] * (2 + i)
        # A layer offset still cannot identify decay with mixed window lengths.
        self.assertEqual(fit.fit_probe_note(*args)[1][0], 1)

    def test_mismatched_window_times_lengths_and_shapes_rejected(self):
        for source in (1, 2, 3):
            for field, value in (("times", np.array([0.06, 0.35, 0.65, 1, 1.4])),
                                 ("window_samples", np.array([5760, 14400, 14400, 14400, 14400])),
                                 ("window_samples", np.array([14400])),
                                 ("power", np.zeros((4, fit.PARTIALS))),
                                 ("confidence", np.zeros((5, fit.PARTIALS - 1)))):
                args = self.fixture()
                setattr(args[0][0][source], field, value)
                with self.subTest(source=source, field=field), self.assertRaisesRegex(ValueError, "window"):
                    fit.fit_probe_note(*args)

    def test_recovers_gain_and_decay_with_nonmonotone_baseline(self):
        args = self.fixture()
        self.assertEqual(args[0][0][1].loss_confidence[0], 0)
        gain, decay, report = fit.fit_probe_note(*args)
        np.testing.assert_allclose(gain[:, 0] - args[2][:, 0], [-3, 2, 6], atol=0.04)
        self.assertAlmostEqual(decay[0], 1.4, delta=0.005)
        np.testing.assert_array_equal(gain[:, 1:], args[2][:, 1:])
        np.testing.assert_array_equal(decay[1:], args[3][1:])
        self.assertTrue(report[0]["response_condition"]["identified"])
        self.assertEqual(report[0]["derivative_observations"], 25)
        self.assertLess(report[0]["predicted_residual_rms_db_after"], 0.04)
        self.assertGreater(report[0]["decay_confidence"], 0.9)
        # A changed fixed global anchor affects gains equally, not decay.
        shifted = list(args)
        shifted[1] += 4
        other_gain, other_decay, _ = fit.fit_probe_note(*shifted)
        np.testing.assert_allclose(other_gain[:, 0] - gain[:, 0], 4, atol=0.04)
        self.assertAlmostEqual(other_decay[0], decay[0], delta=0.005)

    def test_constant_response_no_response_and_no_perturbation_refused(self):
        for derivative, factor, scale in (([4] * 5, 0.7, 1), ([0] * 5, 0.7, 1),
                                           (None, 1, 1), (None, 1.000001, 1), (None, 0.7, 0.1),
                                           ([1000, 1001, 1002, 1003, 1004], 0.7, 1)):
            with self.subTest(derivative=derivative, factor=factor, scale=scale):
                args = self.fixture(derivative=derivative, factor=factor, previous_scale=scale)
                _, decay, report = fit.fit_probe_note(*args)
                self.assertEqual(decay[0], scale)
                self.assertEqual(report[0]["decay_confidence"], 0)
        args = self.fixture()
        for i, (_, model, probe, _) in enumerate(args[0]):
            # Layer-specific constants still cannot distinguish gain from decay.
            probe.power[:, 0] = model.power[:, 0] * (2 + i)
        self.assertEqual(fit.fit_probe_note(*args)[1][0], 1)

    def test_time_span_and_noise_cannot_identify_decay(self):
        for source in (1, 2, 3):
            args = self.fixture()
            for triple in args[0]:
                triple[source].confidence[:, 0] = 0
            gain, decay, report = fit.fit_probe_note(*args)
            self.assertEqual(decay[0], 1)
            if source != 2:  # A missing probe does not prohibit supported gain-only fitting.
                self.assertTrue(np.all(gain[:, 0] <= args[2][:, 0]))
            self.assertEqual(report[0]["decay_confidence"], 0)
        args = self.fixture()
        for _, _, _, reference in args[0]:
            reference.confidence[2:, 0] = 0
        self.assertEqual(fit.fit_probe_note(*args)[1][0], 1)

    def test_robust_fit_and_applied_bounds_compensation(self):
        args = self.fixture(velocities=fit.REPRESENTATIVE_VELOCITIES)
        args[0][4][3].power[2, 0] *= 100  # One 20 dB reference outlier.
        gain, decay, _ = fit.fit_probe_note(*args)
        np.testing.assert_allclose(gain[:, 0] - args[2][:, 0], [-3, 2, 6], atol=0.25)
        self.assertAlmostEqual(decay[0], 1.4, delta=0.015)
        for ratio, scale, expected in ((4, 1, 2), (0.1, 1, 0.5), (3, 3, 4), (0.1, 0.15, 0.1)):
            args = self.fixture(ratio=ratio, previous_scale=scale, gains=(35, -40, 50))
            gain, decay, report = fit.fit_probe_note(*args)
            self.assertAlmostEqual(decay[0], expected, places=6)
            self.assertTrue(report[0]["decay_clamped"])
            self.assertTrue(np.all(abs(gain - args[2]) <= 12))
            self.assertTrue(np.all((gain >= -36) & (gain <= 24)))
            d = np.asarray(report[0]["derivative_db_per_log_scale"])
            target = np.array([10 * np.log10(r.power[:, 0] / m.power[:, 0]) + args[1]
                               for _, m, _, r in args[0]])
            prediction = (fit.velocity_weights([v for v, *_ in args[0]]) @ (gain[:, 0] - args[2][:, 0]))[:, None]
            prediction = prediction + d * np.log(decay[0] / scale)
            np.testing.assert_allclose(report[0]["predicted_residual_db"], target - prediction)

    def test_noise_guards_cut_bounds_and_missing_soft_knot(self):
        args = self.fixture(gains=(10, 10, 10), ratio=1)
        args[0][0][1].confidence[:, 0] = 0
        gain, _, report = fit.fit_probe_note(*args)
        self.assertEqual(gain[0, 0], args[2][0, 0])
        self.assertEqual(report[0]["knot_boost_evidence_factors"][0], 0)
        args = self.fixture(gains=(10, 10, 10), ratio=1)
        ref = args[0][1][3]  # An intermediate censored reference guards both adjacent knots.
        ref.confidence[:, 0] = 0
        ref.power_upper_bound[:, 0] *= 0.001
        gain, _, report = fit.fit_probe_note(*args)
        self.assertEqual(report[0]["knot_cut_only"], [True, True, False])
        self.assertTrue(np.all(gain[:2, 0] <= args[2][:2, 0]))
        self.assertGreater(gain[2, 0] - args[2][2, 0], 9)
        args = self.fixture()
        for _, _, _, reference in args[0]:
            reference.confidence[:, 0] = 0
            reference.power_upper_bound[:, 0] *= 0.001
        gain, decay, report = fit.fit_probe_note(*args)
        self.assertTrue(np.all(gain[:, 0] < args[2][:, 0]))
        self.assertEqual(decay[0], 1)
        self.assertTrue(all(a <= b for a, b in zip(report[0]["cut_only_residual_db_after"],
                                                  report[0]["cut_only_residual_db_before"])))
        for _, _, _, reference in args[0]:
            reference.location_confidence[0] = 0
        np.testing.assert_array_equal(fit.fit_probe_note(*args)[0], args[2])
        for _, _, _, reference in args[0]:
            reference.location_confidence[0] = 1
        # A reference ceiling above the model is not a gain target either.
        shifted = list(args)
        shifted[1] += 100
        np.testing.assert_array_equal(fit.fit_probe_note(*shifted)[0], args[2])


class VelocityAndOutputTests(unittest.TestCase):
    def test_legacy_64_row_table_rejected_clearly(self):
        table = ",".join(fit.CSV_FIELDS) + "\n"
        table += "".join(f"63,{partial},0,0,0,1\n" for partial in range(1, 65))
        with self.assertRaisesRegex(ValueError, f"exactly {fit.PARTIALS} partials.*legacy 64-row"):
            fit.read_calibration(table, "legacy.csv")

    def test_velocity_weights_and_all_layers_used(self):
        weights = fit.velocity_weights([1, 28, 48, 68, 90, 112, 127])
        np.testing.assert_allclose(weights, [[1, 0, 0], [1, 0, 0], [.5, .5, 0], [0, 1, 0], [0, .5, .5], [0, 0, 1], [0, 0, 1]])
        velocities = fit.REPRESENTATIVE_VELOCITIES
        truth = np.array([-4, 2, 7])
        target = fit.velocity_weights(velocities) @ truth
        actual = fit.fit_velocity_gains(velocities, target, np.ones(len(velocities)))
        np.testing.assert_allclose(actual, truth, atol=0.06)
        perturbed = target.copy()
        perturbed[2] += 10
        changed = fit.fit_velocity_gains(velocities, perturbed, np.ones(len(velocities)))
        self.assertGreater(np.max(abs(changed - actual)), 0.5)
        np.testing.assert_array_equal(fit.fit_velocity_gains(fit.KNOTS, np.full(3, 1000), np.zeros(3)), np.zeros(3))

    def test_sorted_csv_rust_shapes_bounds_and_roundtrip(self):
        notes = {63: (np.full((3, fit.PARTIALS), -36.0), np.full(fit.PARTIALS, 0.1)),
                 21: (np.full((3, fit.PARTIALS), 24.0), np.full(fit.PARTIALS, 4.0))}
        with tempfile.TemporaryDirectory(dir=CHECKOUT) as directory:
            fit.write_outputs(directory, notes, {}, {})
            directory = Path(directory)
            with (directory / "calibration.csv").open() as source:
                reader = csv.DictReader(source)
                self.assertEqual(reader.fieldnames, list(fit.CSV_FIELDS))
                rows = list(reader)
            self.assertEqual([(int(r["key"]), int(r["partial"])) for r in rows], [(k, p) for k in (21, 63) for p in range(1, fit.PARTIALS + 1)])
            rust = (directory / "calibration_data.rs").read_text()
            self.assertIn("use super::CalibrationNote;", rust)
            self.assertIn("pub const DEFAULT_CALIBRATION: &[CalibrationNote]", rust)
            self.assertEqual(re.findall(r"key: (\d+)", rust), ["21", "63"])
            arrays = re.findall(r"\[(-?\d+\.\d+(?:, -?\d+\.\d+)*)\]", rust)
            self.assertEqual(len(arrays), 8)
            self.assertTrue(all(len(array.split(", ")) == fit.PARTIALS for array in arrays))
            restored = fit.read_previous(directory / "calibration.csv")
            for key in notes:
                np.testing.assert_array_equal(restored[key][0], notes[key][0])
                np.testing.assert_array_equal(restored[key][1], notes[key][1])
            before = {p.name: p.read_bytes() for p in directory.iterdir()}
            fit.write_outputs(directory, notes, {}, {})
            self.assertEqual(before, {p.name: p.read_bytes() for p in directory.iterdir()})
            bad = {21: (np.full((3, fit.PARTIALS), np.nan), np.ones(fit.PARTIALS))}
            with self.assertRaisesRegex(ValueError, "invalid calibration"):
                fit.write_outputs(directory, bad, {}, {})


def probe_fixture(root, renders, velocities=(28, 68, 84, 112)):
    previous_dir, probe_dir = root / "previous", root / "probe"
    probe_dir.mkdir()
    notes = {key: (np.full((3, fit.PARTIALS), 1.25), np.resize([0.1, 0.12, 0.923456, 2.0, 4.0], fit.PARTIALS))
             for key in (60, 63)}
    fit.write_outputs(previous_dir, notes, {"anchor": {"offset_db": -6, "method": "fixed"}}, {})
    previous_path = previous_dir / "calibration.csv"
    # Parse the rounded previous CSV before computing the perturbation.
    previous = fit.read_previous(previous_path)
    probe_notes = {k: (g, np.clip(d * 0.7, *fit.DECAY_LIMITS)) for k, (g, d) in previous.items()}
    fit.write_outputs(root / "probe-table", probe_notes, {}, {})
    baseline = {"schema": 1, "mode": "calibration", "rate_hz": 48000, "seconds": 4,
                "notes": [60, 63], "velocities": list(velocities), "note_on_sample": 0,
                "note_off": None, "block_frames": 256, "dry": True, "effects": "dry",
                "design_defaults_plus_overrides": {"rad_hp1": 90},
                "calibration": {"path": str(previous_path), "csv": previous_path.read_text()}}
    probe = copy.deepcopy(baseline)
    probe["calibration"] = {"path": "probe.csv", "csv": (root / "probe-table" / "calibration.csv").read_text()}
    for directory, metadata in ((renders, baseline), (probe_dir, probe)):
        (directory / "render.json").write_text(json.dumps(metadata))
        for key in baseline["notes"]:
            for velocity in velocities:
                (directory / f"note_{key:03}_vel_{velocity:03}.wav").touch()
    return previous_path, probe_dir, baseline, probe, previous


class ProbeProvenanceTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=CHECKOUT)
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.renders = self.root / "renders"
        self.renders.mkdir()
        self.path, self.probe_dir, self.baseline, self.probe, self.previous = probe_fixture(self.root, self.renders)

    def validate(self, baseline=None, probe=None, factor=0.7):
        return fit.validate_probe(self.renders, self.baseline if baseline is None else baseline,
                                  self.probe_dir, self.probe if probe is None else probe, self.previous, factor)

    def test_validates_every_csv_row_with_rounding_and_actual_clamp(self):
        table = self.validate()
        self.assertEqual(table[60][1][0], 0.1)
        self.assertAlmostEqual(table[60][1][2], 0.923456 * 0.7, delta=5.1e-7)
        for which in ("baseline", "probe"):
            for field in ("pp_db", "mf_db", "ff_db", "decay_scale"):
                info = copy.deepcopy(getattr(self, which))
                rows = list(csv.DictReader(io.StringIO(info["calibration"]["csv"])))
                rows[-1][field] = str(float(rows[-1][field]) + 0.01)
                text = io.StringIO()
                writer = csv.DictWriter(text, fit.CSV_FIELDS)
                writer.writeheader()
                writer.writerows(rows)
                info["calibration"]["csv"] = text.getvalue()
                with self.subTest(which=which, field=field), self.assertRaises(ValueError):
                    self.validate(**{which: info})
        for text in ("", self.probe["calibration"]["csv"].rsplit("\n", 2)[0],
                     self.probe["calibration"]["csv"] + self.probe["calibration"]["csv"].splitlines()[1] + "\n"):
            bad = copy.deepcopy(self.probe)
            bad["calibration"]["csv"] = text
            with self.assertRaises(ValueError):
                self.validate(probe=bad)
        for factor in (0, -0.7, 1, float("inf"), float("nan")):
            with self.assertRaises(ValueError):
                self.validate(factor=factor)

    def test_settings_missing_metadata_and_file_inventory_rejected(self):
        for field, value in (("dry", False), ("effects", "wet"), ("mode", "stock"),
                             ("rate_hz", 44100), ("seconds", 5), ("block_frames", 128),
                             ("notes", [60]), ("velocities", [28, 68, 112]),
                             ("design_defaults_plus_overrides", {"rad_hp1": 120}),
                             ("note_off", 48000), ("calibration", {"path": "unverified.csv"})):
            with self.subTest(field=field):
                bad = {**self.probe, field: value}
                with self.assertRaises(ValueError):
                    self.validate(probe=bad)
        with self.assertRaises(ValueError):
            self.validate(probe={"calibration": self.probe["calibration"]})
        with self.assertRaises(ValueError):
            self.validate(probe={**self.probe, "unknown_voicing_setting": 1})
        missing = self.probe_dir / "note_060_vel_068.wav"
        missing.unlink()
        with self.assertRaisesRegex(ValueError, "WAV notes/velocities"):
            self.validate()
        missing.touch()
        (self.probe_dir / "note_060_vel_099.wav").touch()
        with self.assertRaisesRegex(ValueError, "WAV notes/velocities"):
            self.validate()


class CliTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=CHECKOUT)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.corpus = self.root / "corpus"
        self.renders = self.root / "renders"
        self.out = self.root / "out"
        self.corpus.mkdir()
        self.renders.mkdir()
        (self.corpus / "48khz24bit").mkdir()
        (self.renders / "render.json").write_text('{"kind":"synthetic"}')
        boundaries = [1, 27, 35, 37, 44, 47, 51, 57, 65, 73, 81, 89, 97, 105, 113, 121, 128]
        sfz = "// synthetic native regions\n<global> trigger=attack\n<group> amp_veltrack=73\n"
        for key, name in ((60, "C4"), (63, "D#4")):
            for layer in range(1, 17):
                sfz += f"<region> sample=48khz24bit\\{name}v{layer}.wav lokey={key - 1} hikey={key + 1} pitch_keycenter={key} lovel={boundaries[layer - 1]} hivel={boundaries[layer] - 1}\n"
        sfz += "<group> trigger=release\n<region> sample=48khz24bit\\rel60.wav key=60\n"
        (self.corpus / "SalamanderGrandPianoV3.sfz").write_text(sfz)

    def invoke(self, *extra):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            fit.main(["--renders", str(self.renders), "--corpus", str(self.corpus), "--out", str(self.out), *extra])

    def seed_audio(self):
        for key, name in ((60, "C4"), (63, "D#4")):
            for velocity, layer in ((28, 2), (68, 9), (112, 14), (84, 11)):
                model = note(key=key, rate=48000, duration=4, gain_db=-4 if key == 63 else 0)
                reference = note(key=key, rate=48000, duration=2, gain_db=6)
                write_wav(self.renders / f"note_{key:03}_vel_{velocity:03}.wav", model, 48000)
                write_wav(self.corpus / "48khz24bit" / f"{name}v{layer}.wav", reference, 48000, 1, 24)

    def test_native_layer_selection_and_no_neighbor_substitution(self):
        _, regions = fit.parse_sfz(self.corpus)
        self.assertEqual(sorted({r.key for r in regions}), [60, 63])
        for velocity, layer in zip(fit.REPRESENTATIVE_VELOCITIES, range(1, 17)):
            self.assertEqual(fit.select_region(regions, 60, velocity).layer, layer)
        self.assertEqual([fit.select_region(regions, 60, v).layer for v in fit.KNOTS], [2, 9, 14])
        with self.assertRaisesRegex(ValueError, "found 0"):
            fit.select_region(regions, 61, 68)
        with self.assertRaises(SystemExit) as error:
            self.invoke("--notes", "60")
        self.assertEqual(error.exception.code, 2)
        self.assertFalse(self.out.exists())

    def test_complete_cli_anchor_manifest_extra_layers_and_previous(self):
        self.seed_audio()
        original = {p: fit.sha256(p) for p in self.renders.iterdir()}
        self.invoke("--notes", "63,60")
        metadata = json.loads((self.out / "metadata.json").read_text())
        summary = json.loads((self.out / "summary.json").read_text())
        self.assertEqual(metadata["keys"], [60, 63])
        self.assertEqual(metadata["partials"], fit.PARTIALS)
        self.assertEqual(metadata["windows_seconds"], [0.06, 0.35, 0.65, 1, 1.4])
        self.assertEqual(metadata["window_durations_seconds"], [0.12, 0.30, 0.30, 0.30, 0.30])
        self.assertEqual(metadata["measurement_settings"]["early_identification_seconds"], [0, 0.15])
        self.assertAlmostEqual(metadata["anchor"]["offset_db"], -6, delta=0.01)
        self.assertEqual(summary["fitted_inputs"], 8)
        for item in summary["notes"]:
            self.assertEqual([i["velocity"] for i in item["inputs"]], [28, 68, 84, 112])
        for item in metadata["source_manifest"]:
            self.assertEqual(item["sha256"], fit.sha256(self.corpus / item["path"]))
        calibrated = fit.read_previous(self.out / "calibration.csv")
        np.testing.assert_allclose(calibrated[60][0][:, :8], 0, atol=0.1)
        np.testing.assert_allclose(calibrated[63][0][:, :8], 4, atol=0.3)  # No per-note normalization.
        self.assertEqual(original, {p: fit.sha256(p) for p in self.renders.iterdir()})
        old_out = self.out
        self.out = self.root / "second"
        self.invoke("--notes", "60", "--previous", str(old_out / "calibration.csv"))
        repeated = json.loads((self.out / "metadata.json").read_text())
        self.assertEqual(repeated["anchor"]["offset_db"], metadata["anchor"]["offset_db"])
        self.assertTrue(repeated["anchor"]["reused_from_previous"])
        self.assertIsNone(repeated.get("per_note_gain"))

    def test_fixed_anchor_pilot_and_missing_extra_velocity(self):
        self.seed_audio()
        (self.renders / "note_060_vel_068.wav").unlink()
        self.invoke("--notes", "63", "--reference-offset-db", "-6")
        with self.assertRaises(SystemExit):
            self.invoke("--notes", "63", "--reference-offset-db", "-6", "--velocities", "28,68,112,124")
        self.out = self.renders
        with self.assertRaises(SystemExit):
            self.invoke("--notes", "63", "--reference-offset-db", "-6")

    def test_probe_cli_anchor_provenance_and_source_preservation(self):
        self.seed_audio()
        previous, probe_dir, _, probe_info, _ = probe_fixture(self.root, self.renders)
        for source in self.renders.glob("*.wav"):
            (probe_dir / source.name).write_bytes(source.read_bytes())
        originals = {p: fit.sha256(p) for directory in (self.renders, probe_dir) for p in directory.iterdir()}
        options = ["--notes", "60,63", "--previous", str(previous),
                   "--decay-probe", str(probe_dir), "--probe-decay-factor", "0.7"]
        # A reused anchor must not consult a newly measured RMS; this fixture has none.
        with mock.patch.object(fit, "analyze", return_value=measured([-20, -25, -22, -31, -24])):
            self.invoke(*options)
            outputs = {p.name: p.read_bytes() for p in self.out.iterdir()}
            self.invoke(*options)
            self.assertEqual(outputs, {p.name: p.read_bytes() for p in self.out.iterdir()})
        metadata = json.loads((self.out / "metadata.json").read_text())
        self.assertEqual(metadata["anchor"]["offset_db"], -6)
        self.assertTrue(metadata["anchor"]["reused_from_previous"])
        self.assertEqual(metadata["probe_metadata"], probe_info)
        self.assertEqual(metadata["probe_decay_factor"], 0.7)
        self.assertEqual(metadata["decay_step_limits"], [0.5, 2])
        self.assertEqual(len(metadata["probe_manifest"]), 9)
        for entry in metadata["probe_manifest"]:
            self.assertEqual(entry["sha256"], fit.sha256(probe_dir / entry["path"]))
        self.assertEqual(originals, {p: fit.sha256(p) for p in originals})
        with self.assertRaises(SystemExit):
            self.invoke(*options, "--reference-offset-db", "-5")
        old_out, self.out = self.out, probe_dir
        with self.assertRaises(SystemExit):
            self.invoke(*options)
        self.out = old_out
        prior_meta = previous.parent / "metadata.json"
        prior_meta.write_text('{"anchor":{"offset_db":-6},"generated_sha256":{}}')
        with self.assertRaises(SystemExit):
            self.invoke(*options)
        with self.assertRaises(SystemExit):
            self.invoke(*options, "--reference-offset-db", "-6")
        prior_meta.unlink()
        with self.assertRaises(SystemExit):
            self.invoke(*options)
        with mock.patch.object(fit, "analyze", return_value=measured([-20, -25, -22, -31, -24])):
            self.invoke(*options, "--reference-offset-db", "-6")
        self.assertEqual(json.loads((self.out / "metadata.json").read_text())["anchor"]["offset_db"], -6)

    def test_probe_cli_requires_previous_and_factor(self):
        for options in (("--decay-probe", "unused"), ("--probe-decay-factor", "0.7"),
                        ("--decay-probe", "unused", "--probe-decay-factor", "0.7")):
            with self.assertRaises(SystemExit):
                self.invoke(*options)
        self.assertFalse(self.out.exists())


if __name__ == "__main__":
    unittest.main()
