#!/usr/bin/env bash
# Installs ~/.dx/tools/wasm-bindgen-*/wasm-bindgen wrapper: strip target_features, run real CLI,
# then patch emitted JS for Makepad (no bare `import … from "env"`).
# Requires: wasm-opt (binaryen), python3, repo tools/patch_makepad_wasm_bindgen_js.py, dx having installed wasm-bindgen once.
set -euo pipefail
ver="${1:-}"
if [[ -z "${ver}" ]]; then
  ver="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)"
fi
if [[ -z "${ver}" ]]; then
  echo "usage: $0 [wasm-bindgen-version] # e.g. 0.2.117" >&2
  exit 1
fi
REPO_TOOLS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCH_PY="${REPO_TOOLS}/patch_makepad_wasm_bindgen_js.py"
dx_tools="${HOME}/.dx/tools"
dir="${dx_tools}/wasm-bindgen-${ver}"
bin="${dir}/wasm-bindgen"
real="${dir}/wasm-bindgen.real"
if [[ ! -x "${bin}" ]]; then
  echo "missing ${bin}; run dx once so Dioxus installs wasm-bindgen ${ver}" >&2
  exit 1
fi
if [[ ! -x "${real}" ]]; then
  mv "${bin}" "${real}"
fi
cat >"${bin}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
PATCH_PY="${PATCH_PY}"
dir="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
real="\${dir}/wasm-bindgen.real"
wasm_in=""
for a in "\$@"; do
  if [[ "\${a}" == *.wasm ]]; then wasm_in="\${a}"; break; fi
done
if [[ -n "\${wasm_in}" && -f "\${wasm_in}" ]]; then
  tmp="\${wasm_in}.makepad_strip_tf.\$\$"
  wasm-opt --strip-target-features "\${wasm_in}" -o "\${tmp}"
  mv "\${tmp}" "\${wasm_in}"
fi
"\${real}" "\$@"
out_dir=""
out_name=""
prev=""
for a in "\$@"; do
  if [[ "\${prev}" == "--out-dir" ]]; then out_dir="\${a}"; fi
  if [[ "\${prev}" == "--out-name" ]]; then out_name="\${a}"; fi
  prev="\${a}"
done
if [[ -n "\${out_dir}" && -n "\${out_name}" && -f "\${PATCH_PY}" ]]; then
  python3 "\${PATCH_PY}" "\${out_dir}/\${out_name}.js"
fi
EOF
chmod +x "${bin}"
echo "installed Makepad wasm-bindgen shim: ${bin}"
