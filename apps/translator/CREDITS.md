# Translator credits

## On-device models

The phone runs these models itself (converted to our formats by our own
tools; downloaded on first run from a paired hub or the mirror):

- Translation and camera reading: Qwen3.5-4B and its vision tower, by the Qwen
  team, Alibaba Cloud. Apache License 2.0.
  https://huggingface.co/Qwen
- Listening: Whisper large-v3 turbo, by OpenAI. MIT License.
  https://github.com/openai/whisper
- Speaking: Kokoro-82M v1.0 and v1.1-zh with their voices, by hexgrad.
  Apache License 2.0. https://huggingface.co/hexgrad
- Finding text lines in camera pictures: PP-OCRv5 mobile text detection, by
  the PaddlePaddle authors (PaddleOCR). Apache License 2.0.
  https://github.com/PaddlePaddle/PaddleOCR,
  https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_det

See libs/ai/NOTICE for the code this app's AI runtime derives from.
