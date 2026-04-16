#!/usr/bin/env python3
"""Post-process wasm-bindgen `--target web` output for Makepad.

1. Removes invalid `import … from "env"` (browsers cannot resolve bare `env`).
2. Injects `imports.env = env` for instantiation.
3. If `env` is omitted, populates it via `init_env` from `wasm_bridge.js` (same as cargo_makepad).

**Import path:** Dioxus emits bindgen under `public/wasm/`; use `../makepad_wasm_bridge/wasm_bridge.js`.
cargo_makepad emits `bindgen.js` next to `makepad_wasm_bridge/`; pass `--sibling-public` for `./…`.
"""
from __future__ import annotations

import json
import re
import sys
import time
from pathlib import Path

# #region agent log
_AGENT_DEBUG_LOG = Path(
    "/Users/wheregmis/Documents/GitHub/makepad/.cursor/debug-7ce2dc.log"
)


def _agent_debug_log(hypothesis_id: str, message: str, data: dict) -> None:
    try:
        payload = {
            "sessionId": "7ce2dc",
            "timestamp": int(time.time() * 1000),
            "hypothesisId": hypothesis_id,
            "location": "patch_makepad_wasm_bindgen_js.py:main",
            "message": message,
            "data": data,
        }
        _AGENT_DEBUG_LOG.parent.mkdir(parents=True, exist_ok=True)
        with _AGENT_DEBUG_LOG.open("a", encoding="utf-8") as f:
            f.write(json.dumps(payload) + "\n")
    except OSError:
        pass


# #endregion


def _header(bridge_import: str) -> str:
    return f"""import {{ init_env, makepad_exports_shell }} from '{bridge_import}';

let __makepad_set_wasm = null;
function __makepad_prepare_env(env) {{
    if (env === undefined || env === null) env = {{}};
    if (typeof env.js_console_log !== 'function') {{
        __makepad_set_wasm = init_env(env);
    }} else {{
        __makepad_set_wasm = null;
    }}
    return env;
}}

"""


def patch_wasm_bindgen_js(src: str, bridge_import: str) -> str:
    if "__makepad_prepare_env" in src:
        return src

    s = src
    s = re.sub(
        r"^import \* as import\d+ from [\"']env[\"']\s*\n",
        "",
        s,
        flags=re.MULTILINE,
    )
    s = s.replace("import * as __wbg_star0 from 'env';", "")
    s = s.replace("import * as __wbg_star0 from \"env\";", "")
    s = s.replace("imports['env'] = __wbg_star0;", "")
    s = re.sub(r"^\s*[\"']env[\"']: import\d+,\s*\n", "", s, flags=re.MULTILINE)

    if "function __wbg_finalize_init" not in s:
        s = s.replace("return wasm;\n}", "return instance;\n}")

    s = s.replace(
        "__wbg_init(module_or_path, memory) {",
        "__wbg_init(module_or_path, env) {let memory;",
    )
    s = s.replace(
        "async function __wbg_init(module_or_path) {",
        "async function __wbg_init(module_or_path, env) {",
    )
    s = s.replace(
        "function initSync(module) {",
        "function initSync(module, env) {",
    )

    lines = []
    for line in s.splitlines(keepends=True):
        trimmed = line.strip()
        if (
            trimmed.startswith("import * as __wbg_star")
            or trimmed.startswith("import*as import")
            or trimmed.startswith("import * as import")
        ) and (
            "from 'env'" in trimmed
            or 'from "env"' in trimmed
            or "from \"env\"" in trimmed
        ):
            continue
        if (trimmed.startswith('"env":') or trimmed.startswith("'env':")) and "import" in trimmed:
            continue
        lines.append(line)
    s = "".join(lines)

    s = _header(bridge_import) + s

    block_new_tmpl = r"""{ind}env = __makepad_prepare_env(env);
{ind}const imports = __wbg_get_imports();
{ind}imports.env = env;"""

    def _sub_const_imports(m: re.Match[str]) -> str:
        ind = m.group(1)
        return block_new_tmpl.format(ind=ind)

    s = re.sub(
        r"^(\s*)const imports = __wbg_get_imports\(\);\s*$",
        _sub_const_imports,
        s,
        flags=re.MULTILINE,
    )

    s = s.replace(
        "const imports=__wbg_get_imports(memory);",
        "env = __makepad_prepare_env(env);\n    const imports=__wbg_get_imports(memory);\n    imports.env=env;",
    )
    s = s.replace(
        "const imports = __wbg_get_imports(memory);",
        "env = __makepad_prepare_env(env);\n    const imports = __wbg_get_imports(memory);\n    imports.env = env;",
    )

    s = re.sub(
        r"^(\s*)imports = __wbg_get_imports\(\);\s*$",
        r"\1env = __makepad_prepare_env(env);\n\1imports = __wbg_get_imports();\n\1imports.env = env;",
        s,
        flags=re.MULTILINE,
    )

    s = re.sub(
        r"function __wbg_finalize_init\(instance, module\) \{\s*"
        r"wasm = instance\.exports;\s*"
        r"wasmModule = module;\s*"
        r"wasm\.__wbindgen_start\(\);\s*"
        r"return wasm;\s*"
        r"\}",
        "function __wbg_finalize_init(instance, module) {\n"
        "    const __makepad_exp = instance.exports;\n"
        "    wasmModule = module;\n"
        "    __makepad_exp.__wbindgen_start();\n"
        "    wasm = makepad_exports_shell(__makepad_exp, module);\n"
        "    return wasm;\n"
        "}",
        s,
        flags=re.MULTILINE,
    )

    fin_old = "return __wbg_finalize_init(instance, module);"
    fin_new = """const __makepad_ret = __wbg_finalize_init(instance, module);
    if (__makepad_set_wasm) { __makepad_set_wasm(__makepad_ret); __makepad_set_wasm = null; }
    return __makepad_ret;"""
    s = s.replace(fin_old, fin_new)

    return s


def main() -> int:
    args = sys.argv[1:]
    sibling_public = False
    if args and args[-1] == "--sibling-public":
        sibling_public = True
        args.pop()
    if len(args) != 1:
        print(
            "usage: patch_makepad_wasm_bindgen_js.py <file.js> [--sibling-public]",
            file=sys.stderr,
        )
        return 2
    path = Path(args[0])
    bridge = (
        "./makepad_wasm_bridge/wasm_bridge.js"
        if sibling_public
        else "../makepad_wasm_bridge/wasm_bridge.js"
    )
    raw = path.read_text(encoding="utf-8")
    out = patch_wasm_bindgen_js(raw, bridge)
    path.write_text(out, encoding="utf-8")
    # #region agent log
    _agent_debug_log(
        "bridge_path",
        "wasm_bindgen_js_patched",
        {
            "js_path": str(path),
            "bridge_import": bridge,
            "sibling_public": sibling_public,
        },
    )
    # #endregion
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
