#!/usr/bin/env python3
"""Build the portable Windows runner on Linux using Rust's bundled LLD.

SDK setup is an explicit separate step:
  cargo build --release -p makepad-loader --no-default-features --bin makepad-builder-cli
  target/release/makepad-builder-cli windows-sdk --root ~/loader-cross/windows --accept-ms-license
  rustup target add x86_64-pc-windows-msvc
  python3 tools/makepad_builder/cross-windows.py --sdk-root ~/loader-cross/windows
No Windows machine, Wine, MSBuild or system-wide SDK install is needed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import zipfile

TARGET = "x86_64-pc-windows-msvc"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sdk-root", required=True, type=Path)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("This build path is for Linux hosts.")
    source = Path(__file__).resolve().parents[2]
    sdk = args.sdk_root.expanduser().resolve() / "sdk"
    target_dir = (args.target_dir or source / "target").resolve()
    output = (args.out or target_dir / "loader-runners" / "windows-x86_64.zip").resolve()
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    host = next(line.removeprefix("host: ") for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    lld = sysroot / "lib/rustlib" / host / "bin/rust-lld"
    if not lld.is_file() or not (sysroot / "lib/rustlib" / TARGET / "lib").is_dir():
        parser.error("Rust's LLD and the Windows standard library are required. Run: rustup target add " + TARGET)
    # The Windows loader has already verified package hashes and unpacked these.
    roots = []
    for parent, sentinel in ((sdk / "VC/Tools/MSVC", "lib/x64/libcmt.lib"),
                             (sdk / "Windows Kits/10/Lib", "um/x64/kernel32.lib"),
                             (sdk / "Windows Kits/10/Lib", "ucrt/x64/libucrt.lib")):
        found = []
        if parent.is_dir():
            for version in parent.iterdir():
                folder = version / Path(sentinel).parent
                if folder.is_dir() and any(p.name.lower() == Path(sentinel).name for p in folder.iterdir()):
                    found.append(folder)
        if len(found) != 1:
            parser.error("Expected one extracted SDK version containing " + sentinel + ". Run windows-sdk first, or use a version-specific SDK root.")
        roots.append(found[0])
    # DEFAULTLIB directives use Windows' case-insensitive names. Provide a
    # lowercase index on Linux without modifying the downloaded SDK payloads.
    libraries = args.sdk_root.expanduser().resolve() / "link-libs"
    libraries.mkdir(parents=True, exist_ok=True)
    for folder in roots:
        for path in folder.iterdir():
            if path.is_file() and path.suffix.lower() == ".lib":
                name = libraries / path.name.lower()
                if name.is_symlink():
                    if name.resolve() == path.resolve():
                        continue
                    parser.error("Conflicting SDK library: " + name.name)
                if name.exists():
                    parser.error("SDK link index contains a non-link: " + str(name))
                name.symlink_to(path)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["MAKEPAD_PACKAGE_DIR"] = "."
    env["CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER"] = str(lld)
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(["-C", "linker-flavor=lld-link", "-C", "target-feature=+crt-static", "-L", "native=" + str(libraries)])
    common = ["-p", "makepad-loader", "--target", TARGET]
    subprocess.run(["cargo", "check"] + common, cwd=source, env=env, check=True)
    # Distribution executables do not carry PDBs. Strip at the final link so
    # LLD does not look for Microsoft's private build-machine debug databases.
    for binary in ("makepad-builder",):
        subprocess.run(["cargo", "rustc", "--release"] + common + ["--bin", binary, "--", "-C", "strip=symbols", "-C", "link-arg=/DEBUG:NONE"], cwd=source, env=env, check=True)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(".zip.part")
    binaries = {}
    with zipfile.ZipFile(temporary, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in ("makepad-builder.exe",):
            executable = target_dir / TARGET / "release" / name
            if executable.read_bytes()[:2] != b"MZ":
                raise RuntimeError("Build did not produce a Windows executable: " + name)
            archive.write(executable, name)
            binaries[name] = hashlib.sha256(executable.read_bytes()).hexdigest()
        # Fonts are runtime assets, not embedded by app_main's manifest. Ship
        # the complete existing widget resource set so the terminal can render
        # international project names and agents' output on a clean machine.
        resources = source / "widgets/resources"
        for asset in sorted(resources.rglob("*")):
            if asset.is_file():
                archive.write(asset, "makepad_widgets/resources/" + asset.relative_to(resources).as_posix())
    temporary.replace(output)
    evidence = {"target": TARGET, "rust": subprocess.check_output(["rustc", "--version"], text=True).strip(),
                "sdk_libraries": [str(p) for p in roots], "binaries": binaries,
                "zip_sha256": hashlib.sha256(output.read_bytes()).hexdigest()}
    output.with_suffix(".build.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print("\nBuilt " + str(output))
    print("Validate this ZIP on Windows before publishing it.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print("Windows cross-build failed: " + str(error), file=sys.stderr)
        sys.exit(1)
