# Credits

- MilkDrop visuals (`libs/milkdrop`): the `.milk` preset format, its equation language and shader dialect are
  Ryan Geiss's MilkDrop (Nullsoft / Winamp). The renderer here is an independent implementation; it contains no
  projectM source and bundles no third-party presets.
- Amp's built-in visualizer presets (`resources/visualizer`) are original to Makepad and under Amp's licence.
  Other presets are never shipped with Amp. On request, the preset browser downloads the "cream of the crop" collection
  curated by the projectM project (https://github.com/projectM-visualizer/presets-cream-of-the-crop) into the
  app's data folder. The presets are the work of their individual authors (the original MilkDrop community: Geiss,
  Rovastar, Flexi, Martin, and many others); each remains under its author's terms. Amp only reads the downloaded
  files and does not redistribute them.
- Spotify is a trademark of Spotify AB. Amp is not affiliated with Spotify; it drives the Spotify desktop app
  that is installed on the machine and ships none of its assets.
- `resources/amp-icon.png`, `resources/icon_1024.png`, `resources/icon.svg`: the Amp app icon (an "A" whose crossbar is a five-bar spectrum), original, made for this project: hand-written SVG, no traced or third-party artwork. `icon_1024.png` and `amp-icon.png` are `sh tools/app_icons/render.sh apps/amp/resources/icon.svg` (macOS QuickLook raster + icon_mask) copied to both names; the per-style variants are `widgets/themes/<style>/icons/amp.svg`.

## AI music (the AI strip, `src/ai_music.rs`)

Amp ships no model and no weights. AI music renders on your own AI hub
nodes, which download the weights on request (pinned by size and SHA-256 in
`libs/ai/hub/registry.json`). The songs are for personal listening; each
file names the model that made it in its tags.

- YuE2-3B and YuE2-Vae, by the Multimodal Art Projection (m-a-p) team:
  https://huggingface.co/m-a-p/YuE2-3B (paper: arXiv 2609.33757). Weights
  under Creative Commons Attribution-NonCommercial 4.0: non-commercial use
  with attribution. The Oobleck VAE and SnakeBeta code parts are MIT
  (Stability AI, NVIDIA). The default model of the AI strip.
- ACE-Step 1.5 XL, by ACE Studio and StepFun: https://huggingface.co/ACE-Step/Ace-Step-1.5-XL.
  Apache License 2.0.
- MiniMax Music 3, by MiniMax: https://huggingface.co/MiniMaxAI/MiniMax-Music-3.
  MiniMax-Music3 Community License (attribution on commercial interfaces,
  an acceptable-use policy and a separate authorisation above a revenue cap).
- The song writer is the person's own Claude Code (Anthropic), run chat-only
  and sandboxed by their AI hub (`libs/ai/hub/src/claude_cli.rs`).
