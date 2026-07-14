#!/usr/bin/env bash
# Fetch the voice models the Game Maker (and other voice-enabled examples) use:
#
#   ggml-large-v3-turbo.bin   Whisper speech-to-text (the F1 push-to-talk mic)
#   kokoro-v1_0.mktts         Kokoro text-to-speech weights (makepad format)
#   bm_fable.mkvoice          the "Fable" voice pack
#
# The .mktts/.mkvoice files are makepad's own flat format: this script downloads
# the public upstream weights from HuggingFace and converts them locally with
# libs/tts/tools/convert_kokoro.py (stdlib-only Python, no torch needed).
#
# Everything lands in the repo root, which is where the loaders look when the
# apps are run from there (see libs/tts/src/kokoro.rs and libs/voice).
#
#   usage: ./download_tts.sh [--with-whisper]   # whisper is ~1.6 GB, opt-in
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

KOKORO_URL="https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/kokoro-v1_0.pth"
FABLE_URL="https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/voices/bm_fable.pt"
WHISPER_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"

fetch() { # <url> <dest>
	if [[ -f "$2" ]]; then
		echo "have $2"
	else
		echo "downloading $2 ..."
		curl -L -C - --fail -o "$2.part" "$1"
		mv "$2.part" "$2"
	fi
}

# ── Kokoro TTS (~330 MB) + the Fable voice ──
if [[ -f kokoro-v1_0.mktts ]]; then
	echo "have kokoro-v1_0.mktts"
else
	fetch "$KOKORO_URL" kokoro-v1_0.pth
	python3 libs/tts/tools/convert_kokoro.py kokoro-v1_0.pth kokoro-v1_0.mktts
fi

if [[ -f bm_fable.mkvoice ]]; then
	echo "have bm_fable.mkvoice"
else
	mkdir -p kokoro_voices
	fetch "$FABLE_URL" kokoro_voices/bm_fable.pt
	python3 libs/tts/tools/convert_kokoro.py --voice kokoro_voices/bm_fable.pt bm_fable.mkvoice
fi

# ── Whisper (speech-to-text, ~1.6 GB) ──
if [[ "${1:-}" == "--with-whisper" ]]; then
	fetch "$WHISPER_URL" ggml-large-v3-turbo.bin
else
	[[ -f ggml-large-v3-turbo.bin ]] && echo "have ggml-large-v3-turbo.bin" ||
		echo "skipping whisper (~1.6 GB) — rerun with --with-whisper to enable the F1 mic"
fi

echo "done. run the app from the repo root so the models resolve."
