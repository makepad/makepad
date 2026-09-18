# Tokenizer parity: HF AutoTokenizer (pinned snapshot) vs llama-tokenize on the produced GGUF
$ErrorActionPreference = 'Continue'
$dir = 'C:\ai\models\qwen38'
$tf = "$dir\tok_test.txt"
$lines = @(
  'Hello, world! The quick brown fox jumps over 12345 lazy dogs.',
  'Streaming JSON {"key": [1, 2.5, null]} plus code: fn main() { println!("hi"); }',
  'Unicode: caf'+[char]0x00E9+' na'+[char]0x00EF+'ve '+[char]0x4E2D+[char]0x6587+[char]0x6D4B+[char]0x8BD5+' '+[char]0xD83D+[char]0xDE80+[char]0xD83C+[char]0xDF0D+' end',
  '<|im_start|>plain text with special-looking tokens<|im_end|> and <think> tags'
)
[System.IO.File]::WriteAllText($tf, ($lines -join "`n"), (New-Object System.Text.UTF8Encoding($false)))
$py = @'
import json, subprocess, sys
dir_ = r"C:\ai\models\qwen38"
tf = dir_ + r"\tok_test.txt"
from transformers import AutoTokenizer
tok = AutoTokenizer.from_pretrained(dir_ + r"\hf")
text = open(tf, encoding="utf-8").read()
hf_ids = tok.encode(text, add_special_tokens=False)
out = subprocess.run([r"C:\ai\qwen38\bin\llama-tokenize.exe", "-m", dir_ + r"\Qwen3.8-27B-Q4_K_M.gguf",
                      "-f", tf, "--ids", "--no-bos"], capture_output=True, text=True)
raw = out.stdout.strip()
try:
    ll_ids = json.loads(raw)
except Exception:
    ll_ids = [int(x) for x in raw.replace("[", " ").replace("]", " ").replace(",", " ").split()]
print("hf_n", len(hf_ids), "llama_n", len(ll_ids))
if hf_ids == ll_ids:
    print("TOKENIZER_PARITY PASS")
else:
    print("TOKENIZER_PARITY FAIL")
    for i, (a, b) in enumerate(zip(hf_ids, ll_ids)):
        if a != b:
            print("first_diff_at", i, "hf", hf_ids[max(0,i-3):i+3], "llama", ll_ids[max(0,i-3):i+3])
            break
    if out.stderr:
        print("stderr_tail", out.stderr[-400:])
'@
Set-Content -Path $dir\tok_parity.py -Value $py -Encoding UTF8
python $dir\tok_parity.py
