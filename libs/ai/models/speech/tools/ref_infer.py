#!/usr/bin/env python3
"""Run the reference Kokoro ONNX on tokens produced by *our* Rust phonemizer.

Ground truth for the port: same text, same tokens, same voice. Anything the Rust
graph produces later gets diffed against this.

    /tmp/kref/bin/python libs/tts/tools/ref_infer.py "Escape the Gummer." out.wav
"""

import struct
import subprocess
import sys

import numpy as np
import onnxruntime as ort

MODEL = "kokoro_ref.onnx"
VOICE = "af_heart.mkvoice"
SAMPLE_RATE = 24_000


def rust_tokens(text):
    """Ask the Rust binary, so the reference sees exactly what Rust will feed it."""
    out = subprocess.run(
        ["cargo", "run", "--release", "--quiet",
         "--manifest-path", "libs/tts/Cargo.toml", "--bin", "g2p_test", "--", "--ids", text],
        capture_output=True, text=True, check=True,
    )
    return [int(x) for x in out.stdout.strip().split(",")]


def load_voice(path):
    """Read the `.mktts` container: one [510, 1, 256] tensor."""
    blob = open(path, "rb").read()
    assert blob[:8] == b"MKTTS\0\0\0", "bad voice magic"
    count = struct.unpack("<I", blob[12:16])[0]
    assert count == 1, count
    at = 16
    name_len = struct.unpack("<I", blob[at:at + 4])[0]
    at += 4 + name_len
    _dtype, ndim = blob[at], blob[at + 1]
    at += 2
    shape = struct.unpack(f"<{ndim}I", blob[at:at + 4 * ndim])
    at += 4 * ndim
    offset, nbytes = struct.unpack("<QQ", blob[at:at + 16])
    data = np.frombuffer(blob[offset:offset + nbytes], dtype=np.float32)
    return data.reshape(shape)


def write_wav(path, samples, rate):
    pcm = np.clip(samples, -1.0, 1.0)
    pcm = (pcm * 32767.0).astype("<i2")
    with open(path, "wb") as out:
        out.write(b"RIFF")
        out.write(struct.pack("<I", 36 + pcm.nbytes))
        out.write(b"WAVEfmt ")
        out.write(struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16))
        out.write(b"data")
        out.write(struct.pack("<I", pcm.nbytes))
        out.write(pcm.tobytes())


def main():
    text = sys.argv[1] if len(sys.argv) > 1 else "Escape the Gummer, a squishy purple blob."
    dst = sys.argv[2] if len(sys.argv) > 2 else "kokoro_ref.wav"

    ids = rust_tokens(text)
    phonemes = len(ids) - 2  # the ids are zero-padded at both ends
    voice = load_voice(VOICE)
    # One style vector per phoneme count. Row `phonemes`, both 128-halves.
    # kokoro/pipeline.py uses `pack[len(ps) - 1]`. Row `phonemes` also sounds
    # fine, which is exactly how this off-by-one survives a listening test.
    style = voice[phonemes - 1]
    print(f"text     : {text}")
    print(f"tokens   : {len(ids)} ids ({phonemes} phonemes)")
    print(f"style row: {phonemes - 1} of {voice.shape[0]}  -> {style.shape}")

    session = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
    waveform = session.run(
        None,
        {
            "input_ids": np.array([ids], dtype=np.int64),
            "style": style.astype(np.float32).reshape(1, 256),
            "speed": np.array([1.0], dtype=np.float32),
        },
    )[0][0]

    print(f"waveform : {waveform.shape[0]} samples "
          f"({waveform.shape[0]/SAMPLE_RATE:.2f}s), peak={np.abs(waveform).max():.4f}")
    write_wav(dst, waveform, SAMPLE_RATE)
    print(f"wrote    : {dst}")

    # A 16k copy so Whisper can score it with the same harness.
    index = np.arange(0, len(waveform), SAMPLE_RATE / 16_000)
    resampled = np.interp(index, np.arange(len(waveform)), waveform)
    write_wav(dst.replace(".wav", "_16k.wav"), resampled, 16_000)
    np.save(dst.replace(".wav", ".npy"), waveform)


if __name__ == "__main__":
    main()
