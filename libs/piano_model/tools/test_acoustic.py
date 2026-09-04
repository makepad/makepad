"""Numerical tests require no corpus, audio devices, or third-party WAV reader."""
from pathlib import Path
import struct
import tempfile
import unittest

import numpy as np
import acoustic as a


class AcousticNumerics(unittest.TestCase):
    def test_antiphase_and_gain_invariance(self):
        rate = 48000
        t = np.arange(rate // 4) / rate
        mono = np.sin(2 * np.pi * 1000 * t) + 0.5 * np.sin(2 * np.pi * 4000 * t)
        stereo = np.column_stack([mono, -mono])
        spec = a.spectrum(stereo, rate)
        self.assertAlmostEqual(a.band(spec, 500, 2000), 0.5, places=7)
        self.assertAlmostEqual(a.band(spec, 2000, 8000), 0.125, places=7)
        expected = a.ratio_db(a.band(spec, 2000, 8000), a.band(spec, 20, 20000))
        for gain in [0.0001, 0.1, 4.0]:
            changed = a.spectrum(stereo * gain, rate)
            self.assertAlmostEqual(a.ratio_db(a.band(changed, 2000, 8000), a.band(changed, 20, 20000)), expected, places=10)
            self.assertEqual(a.onset_index(stereo * gain, rate), a.onset_index(stereo, rate))

    def test_pcm24_sign_and_scale(self):
        values = [-8388608, -8388607, -1, 0, 1, 8388607]
        payload = b"".join((n & 0xFFFFFF).to_bytes(3, "little") for n in values)
        rate, decoded = self.wav_roundtrip(1, 24, 2, payload)
        self.assertEqual(rate, 48000)
        np.testing.assert_array_equal(decoded, np.array(values).reshape(-1, 2) / 8388608.0)

    def test_pcm16_and_float32(self):
        _, decoded = self.wav_roundtrip(1, 16, 1, struct.pack("<hhh", -32768, 0, 32767))
        np.testing.assert_array_equal(decoded[:, 0], [-1, 0, 32767 / 32768])
        _, decoded = self.wav_roundtrip(3, 32, 2, struct.pack("<ffff", -1.25, 0.5, 0, 2))
        np.testing.assert_array_equal(decoded, [[-1.25, 0.5], [0, 2]])

    def wav_roundtrip(self, kind, bits, channels, payload):
        align = channels * bits // 8
        fmt = struct.pack("<HHIIHH", kind, channels, 48000, 48000 * align, align, bits)
        # Odd unknown chunk exercises RIFF padding, not a fixed 44-byte reader.
        body = b"WAVEJUNK" + struct.pack("<I", 1) + b"x\0fmt " + struct.pack("<I", len(fmt)) + fmt
        body += b"data" + struct.pack("<I", len(payload)) + payload + (b"\0" if len(payload) % 2 else b"")
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temp:
            path = Path(temp) / "signal.wav"
            path.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)
            return a.read_wav(path)

    def test_sfz_native_velocity_mapping(self):
        regions = a.sfz_regions(r"""
            // actual V3 velocity bounds, including the easy-to-mislabel 28
            <group> amp_veltrack=73
            <region> sample=48khz24bit\C4v2.wav lokey=59 hikey=61 lovel=27 hivel=34
            <region> sample=48khz24bit\C4v4.wav lokey=59 hikey=61 pitch_keycenter=60 lovel=37 hivel=43
            <region> sample=48khz24bit\C4v9.wav lokey=59 hikey=61 pitch_keycenter=60 lovel=65 hivel=72
            <region> sample=48khz24bit\C4v14.wav lokey=59 hikey=61 pitch_keycenter=60 lovel=105 hivel=112
            <region> sample=48khz24bit\C4v16.wav lokey=59 hikey=61 pitch_keycenter=60 lovel=121
            <group> trigger=release
            <region> sample=release.wav pitch_keycenter=60
        """)
        for velocity, layer in [(28, 2), (34, 2), (37, 4), (68, 9), (112, 14), (127, 16)]:
            region = a.native_region(regions, 60, velocity)
            self.assertTrue(region["sample"].endswith(f"v{layer}.wav"))
            self.assertEqual(region["amp_veltrack"], "73")
        with self.assertRaisesRegex(ValueError, "one native"):
            a.native_region(regions, 61, 28)  # in key range, but transposed
        with self.assertRaisesRegex(ValueError, "one native"):
            a.native_region(regions, 60, 35)  # unmapped, no guessing

    def test_onset_alignment_and_decay_units(self):
        rate = 48000
        t = np.arange(round(2.2 * rate)) / rate
        mono = np.sin(2 * np.pi * 1000 * t) * np.exp(-t)
        x = np.column_stack([mono, -mono])
        x = np.concatenate([np.zeros((480, 2)), x])
        result = a.measure(x, rate, 60)
        self.assertEqual(result["onset_sample"], 480)
        self.assertAlmostEqual(result["metrics"]["decay_mid_db_s"], 20 / np.log(10), places=6)
        scaled = a.measure(x * 0.001, rate, 60)
        for key in result["metrics"]:
            self.assertAlmostEqual(result["metrics"][key], scaled["metrics"][key], places=6)
        with self.assertRaisesRegex(ValueError, "silent"):
            a.onset_index(np.zeros((48000, 2)), rate)

    def test_missing_corpus_is_error(self):
        with self.assertRaisesRegex(ValueError, "missing Salamander corpus"):
            a.reference(Path(__file__) / "absent-corpus", a.NOTES, a.VELOCITIES)


if __name__ == "__main__":
    unittest.main()
