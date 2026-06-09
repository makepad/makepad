# Makepad Example: ComfyUI

A Makepad example that uses an OpenAI-compatible LLM to generate image prompts and then submits them to ComfyUI.

## How to Run

1. Check the hardcoded endpoints in [`src/app.rs`](./src/app.rs):
   - `llm_base` defaults to `http://10.0.0.168:8080`
   - `comfy_ip` defaults to `10.0.0.217:8000`
   - `self_ip` and `displays` should match your local network

2. Run the `makepad-example-comfyui` runnable item from Makepad Studio.
