"""Portable Makepad terminal installer. Only explicit menu actions install software."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import select
import termios
import textwrap
from contextlib import contextmanager
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse

if sys.version_info < (3, 9):
    raise SystemExit("Python 3.9 or newer is required.")
ROOT = Path(sys.argv[1]).resolve()
bootstrap = ROOT / "makepad-builder.json"
if not bootstrap.exists():
    bootstrap = ROOT / "makepad-loader.json"
BOOT = json.loads(bootstrap.read_text())
APP = BOOT["app"]
EMAIL = BOOT["email"]
ORIGIN = "https://makepad.nl"
IDENT = re.compile(r"[A-Za-z0-9_-]{1,128}\Z")
if not IDENT.fullmatch(APP) or not isinstance(EMAIL, str) or len(EMAIL) > 254 or ("@" not in EMAIL and not (APP == "makepad" and EMAIL == "")) or any(ord(c) < 32 for c in EMAIL):
    raise SystemExit("Invalid Makepad Builder bootstrap.")
APPS = [{"id": "scope", "title": "Scope"}] + json.loads((ROOT / "apps.json").read_text())
selected = ROOT / "selected-app"
APP = os.environ.get("MAKEPAD_BUILDER_APP", selected.read_text() if selected.is_file() else "calculator" if APP == "makepad" else APP)
if APP not in {a["id"] for a in APPS}:
    raise SystemExit("Unknown selected app.")

def installed_path():
    return ROOT / "installed" / (APP + ".json")

def source_root(release):
    return ROOT / "sources" / release["release"]

def repo_ready(release, repo):
    receipt = source_root(release) / ".builder-repositories" / repo["name"]
    return receipt.is_file() and receipt.read_text() == repo["commit"] + " " + repo["sha256"] and (source_root(release) / repo["path"] / ".git").is_dir()

SYSTEM = platform.system()
ARCH = {"x86_64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}.get(platform.machine())
if ARCH is None or SYSTEM not in ("Darwin", "Linux"):
    raise SystemExit("Makepad Builder supports macOS and Linux on x86_64 and ARM64.")
if SYSTEM == "Linux" and platform.libc_ver()[0] != "glibc":
    raise SystemExit("This release supports x86_64 and ARM64 Linux with glibc. Musl/Alpine is not supported yet.")
TRIPLE = ARCH + ("-apple-darwin" if SYSTEM == "Darwin" else "-unknown-linux-gnu")
PROJECT = Path(os.environ.get("MAKEPAD_LOADER_PROJECT", ROOT)).resolve()
MENU = (ROOT / "menu.txt").read_text().splitlines()
ABOUT = (ROOT / "about.txt").read_text().strip()
COLOR = sys.stdout.isatty() and os.environ.get("TERM", "") != "dumb" and "NO_COLOR" not in os.environ

def ansi(style):
    palette = {"30": "38;2;0;0;0", "32": "38;2;0;100;0", "90": "38;2;112;112;112", "31": "38;2;128;0;0", "37": "38;2;192;192;192",
               "93": "38;2;255;255;85", "97": "38;2;255;255;255",
               "shadow": "48;2;0;0;92", "40": "48;2;0;0;0", "44": "48;2;0;0;128", "47": "48;2;192;192;192"}
    return "\033[0;" + ";".join(palette.get(code, code) for code in style.split(";")) + "m"

def ask(prompt, info=None, title="Confirm compiler setup"):
    if not COLOR:
        for value in info or []:
            print(value)
        return input(prompt + " [Y/n] ").strip().lower() in ("", "y", "yes")
    return menu(title, (info or []) + [prompt], [("y", "Yes, accept and continue", "Enter confirms this step"), ("n", "No, return to setup", "Nothing will be installed")]) == "y"

def download(url, destination=None, expected=None, limit=2 * 1024**3, headers=None):
    # Never print a URL or a remote response body: a query can contain the email.
    digest = hashlib.sha256()
    received = 0
    data = bytearray() if destination is None else None
    part = destination.with_suffix(destination.suffix + ".part") if destination else None
    output = None
    process = None
    last_draw = 0
    try:
        if part:
            part.parent.mkdir(parents=True, exist_ok=True)
            output = part.open("wb")
        # curl uses the platform's TLS setup, including Apple's trust store.
        # A private stdin config keeps the email out of process arguments.
        # Disable curlrc and redirects; never emit a remote error body or URL.
        config = ['silent', 'fail', 'proto = "=https"', 'connect-timeout = 20',
                  'speed-time = 60', 'speed-limit = 1', 'max-time = 900',
                  'user-agent = "Makepad-Builder/0.1"', 'write-out = "%{stderr}%{http_code}"',
                  'url = ' + json.dumps(url), 'header = "Cache-Control: no-store"']
        config.extend('header = ' + json.dumps(k + ": " + v) for k, v in (headers or {}).items())
        process = subprocess.Popen(["curl", "--disable", "--config", "-"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        process.stdin.write(("\n".join(config) + "\n").encode())
        process.stdin.close()
        while True:
            chunk = process.stdout.read(256 * 1024)
            if not chunk:
                break
            received += len(chunk)
            if received > limit:
                raise RuntimeError("Download exceeds its size limit.")
            digest.update(chunk)
            if output:
                output.write(chunk)
                now = time.monotonic()
                if now - last_draw > .1:
                    PROGRESS.update("Download", destination.name, received, expected[0] if expected else 0, "bytes")
                    last_draw = now
            else:
                data.extend(chunk)
        result = process.wait()
        status = process.stderr.read().decode(errors="replace")
        if status != "200" or result:
            if result == 60:
                raise RuntimeError("HTTPS certificate verification failed. Check your system clock and certificate store.")
            if status in ("401", "403"):
                raise RuntimeError("Download refused (HTTP " + status + "). Check app access and the remaining download count in /admin.")
            raise RuntimeError("Download failed" + (" (HTTP " + status + ")" if re.fullmatch(r"[1-5][0-9]{2}", status) else "") + ". Check the connection and retry.")
        if expected and (received != expected[0] or digest.hexdigest() != expected[1].lower()):
            raise RuntimeError("Download failed size/hash verification.")
        if output:
            output.close()
            part.replace(destination)
        return bytes(data) if data is not None else digest.hexdigest()
    finally:
        if process:
            if process.poll() is None:
                process.terminate()
                process.wait()
            process.stdout.close()
            process.stderr.close()
        if last_draw:
            print()
        if output:
            output.close()
        if part and part.exists():
            part.unlink()

def relative(value):
    return isinstance(value, str) and value and "\\" not in value and not value.startswith("/") and all(p not in ("", ".", "..") for p in value.split("/"))

def latest():
    global EMAIL
    PROGRESS.update("Checking sources", "Latest available release", force=True)
    if APP == "scope":
        if not EMAIL:
            leave_screen()
            EMAIL = input("Scope access email: ").strip()
            if "@" not in EMAIL or len(EMAIL) > 254 or any(ord(c) < 32 for c in EMAIL):
                raise RuntimeError("Invalid access email.")
        data = download(ORIGIN + "/api/loader/catalog", limit=1024 * 1024, headers={"X-Makepad-Email": EMAIL})
        release = next((r for r in json.loads(data) if r.get("id") == APP), None)
    else:
        data = download(ORIGIN + "/api/loader/public/catalog", limit=1024 * 1024)
        release = next((r for r in json.loads(data) if r.get("public")), None)
        if release:
            release.update(next(a for a in APPS if a["id"] == APP))
    if release is None or TRIPLE not in release.get("platforms", []):
        raise RuntimeError("No release is published for this app and architecture yet.")
    if not all(IDENT.fullmatch(release[k]) for k in ("id", "release", "package", "binary")) or not relative(release["workspace"]) or not re.fullmatch(r"\d+\.\d+\.\d+", release["rust"]):
        raise RuntimeError("Invalid release metadata.")
    repos = release["repositories"]
    if not 1 <= len(repos) <= 8 or len({r["path"] for r in repos}) != len(repos):
        raise RuntimeError("Invalid repository list.")
    for repo in repos:
        if not IDENT.fullmatch(repo["name"]) or not relative(repo["path"]) or not re.fullmatch(r"[0-9a-fA-F]{40}", repo["commit"]) or not re.fullmatch(r"[0-9a-fA-F]{64}", repo["sha256"]) or not 0 < repo["bytes"] <= 2 * 1024**3:
            raise RuntimeError("Invalid repository metadata.")
    return release

def dependency_packages():
    distro = {}
    for line in (Path("/etc/os-release").read_text().splitlines() if Path("/etc/os-release").is_file() else []):
        if "=" in line:
            key, value = line.split("=", 1)
            distro[key] = value.strip('"\'')
    family = (distro.get("ID", "") + " " + distro.get("ID_LIKE", "")).split()
    common = ["git", "curl", "ca-certificates", "cmake"]
    if any(x in family for x in ("debian", "ubuntu")):
        command = ["apt-get", "install", "--no-install-recommends"] + common + ["build-essential", "clang", "pkg-config", "python3", "libssl-dev", "libx11-dev", "libxcursor-dev", "libxkbcommon-dev", "libxrandr-dev", "libxi-dev", "libxinerama-dev", "libasound2-dev", "libpulse-dev", "libwayland-dev", "wayland-protocols", "libegl-dev", "libgl-dev", "libglx-dev", "libdrm-dev", "libgbm-dev"]
    elif any(x in family for x in ("fedora", "rhel", "centos")):
        command = ["dnf", "install"] + common + ["gcc", "gcc-c++", "make", "clang", "pkgconf-pkg-config", "python3", "openssl-devel", "libX11-devel", "libXcursor-devel", "libxkbcommon-devel", "libXrandr-devel", "libXi-devel", "libXinerama-devel", "alsa-lib-devel", "pulseaudio-libs-devel", "wayland-devel", "wayland-protocols-devel", "libglvnd-devel", "libdrm-devel", "mesa-libgbm-devel"]
    elif "arch" in family:
        command = ["pacman", "-S", "--needed"] + common + ["base-devel", "clang", "pkgconf", "python", "openssl", "libx11", "libxcursor", "libxkbcommon", "libxrandr", "libxi", "libxinerama", "alsa-lib", "libpulse", "wayland", "wayland-protocols", "libglvnd", "mesa", "libdrm"]
    elif any(x in family for x in ("suse", "opensuse", "opensuse-tumbleweed", "opensuse-leap")):
        command = ["zypper", "install"] + common + ["gcc", "gcc-c++", "make", "clang", "pkg-config", "python3", "libopenssl-devel", "libX11-devel", "libXcursor-devel", "libxkbcommon-devel", "libXrandr-devel", "libXi-devel", "libXinerama-devel", "alsa-devel", "libpulse-devel", "wayland-devel", "wayland-protocols-devel", "libglvnd-devel", "libdrm-devel", "Mesa-libgbm-devel"]
    else:
        raise RuntimeError("Unknown distro. Install a C/C++ compiler, Git, CMake, pkg-config, OpenSSL, X11, Xcursor, xkbcommon, ALSA, PulseAudio, Wayland, EGL/GLX and GBM development packages manually.")
    return distro.get("PRETTY_NAME", "Linux"), ["sudo"] + command

def toolchain(release):
    version = release["rust"]
    target = ROOT / "toolchain" / "rust" / (version + "-" + TRIPLE)
    stamp = target / ".toolchain-version"
    if stamp.is_file() and stamp.read_text() == version + " " + TRIPLE and all((target / ("bin/" + name)).is_file() for name in ("cargo", "rustc")):
        return target
    manifest = download("https://static.rust-lang.org/dist/channel-rust-" + version + ".toml", limit=8 * 1024**2).decode()
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".rust-", dir=target.parent) as temporary:
        temporary = Path(temporary)
        ready = temporary / "ready"
        ready.mkdir()
        for index, component in enumerate(("rustc", "rust-std", "cargo"), 1):
            PROGRESS.package("Rust", component, index, 3)
            header = "[pkg." + component + ".target." + TRIPLE + "]"
            section = manifest.split(header, 1)[1].split("\n[", 1)[0]
            url = re.search(r'^url\s*=\s*"([^"]+)"', section, re.M).group(1)
            expected_hash = re.search(r'^hash\s*=\s*"([0-9a-f]{64})"', section, re.M).group(1)
            if not url.startswith("https://static.rust-lang.org/dist/"):
                raise RuntimeError("Unexpected Rust archive origin.")
            archive = temporary / (component + ".tar.gz")
            PROGRESS.update("Download", component)
            if download(url, archive) != expected_hash:
                raise RuntimeError("Rust archive hash mismatch.")
            unpack = temporary / component
            unpack.mkdir()
            PROGRESS.update("Unpacking", component)
            with tarfile.open(archive) as tar:
                for member in tar.getmembers():
                    destination = (unpack / member.name).resolve()
                    if not destination.is_relative_to(unpack) or member.isdev() or member.isfifo():
                        raise RuntimeError("Unsafe Rust archive entry.")
                    if member.issym() or member.islnk():
                        linked = (destination.parent / member.linkname if member.issym() else unpack / member.linkname).resolve()
                        if not linked.is_relative_to(unpack):
                            raise RuntimeError("Unsafe Rust archive link.")
                members = tar.getmembers()
                for index, member in enumerate(members):
                    tar.extract(member, unpack)
                    PROGRESS.update("Unpacking", member.name, index + 1, len(members), "files")
            inner = next(p for p in unpack.iterdir() if p.is_dir())
            component_dir = inner / component if component != "rust-std" else next(inner.glob("rust-std-*"))
            PROGRESS.update("Installing", component)
            shutil.copytree(component_dir, ready, dirs_exist_ok=True, symlinks=True)
        check = subprocess.check_output([str(ready / "bin/rustc"), "--version"], text=True)
        if not check.startswith("rustc " + version + " "):
            raise RuntimeError("Private Rust failed its version check.")
        (ready / ".toolchain-version").write_text(version + " " + TRIPLE)
        if target.exists():
            raise RuntimeError("Incomplete compiler folder already exists: " + str(target))
        ready.rename(target)
    PROGRESS.update("Ready", "Rust installed and verified", force=True)
    return target

def checkout(release):
    destination = source_root(release)
    if all(repo_ready(release, r) for r in release["repositories"]):
        return destination / release["workspace"]
    if not shutil.which("git"):
        raise RuntimeError("Git is missing. Choose Download compiler first.")
    receipts = destination / ".builder-repositories"
    receipts.mkdir(parents=True, exist_ok=True)
    for index, repo in enumerate(sorted(release["repositories"], key=lambda r: len(r["path"])), 1):
        if repo_ready(release, repo):
            continue
        final = destination / repo["path"]
        if final.exists() or not final.resolve().is_relative_to(destination):
            raise RuntimeError("Existing unverified source was left unchanged: " + str(final))
        PROGRESS.package("Source repositories", repo["name"], index, len(release["repositories"]))
        if release.get("public"):
            if repo["name"] != "makepad" or repo["path"] != "makepad":
                raise RuntimeError("Public source may contain only Makepad.")
            url = ORIGIN + "/api/loader/public/source/" + release["release"] + "/makepad.pack"
        else:
            url = ORIGIN + "/" + APP + "/git?" + urllib.parse.urlencode({"email": EMAIL, "release": release["release"], "repository": repo["name"]})
        with tempfile.TemporaryDirectory(prefix=".source-", dir=destination) as temporary:
            stage = Path(temporary)
            pack = stage / "source.pack"
            download(url, pack, (repo["bytes"], repo["sha256"]))
            folder = stage / "checkout"
            folder.mkdir()
            git = ["git", "-C", str(folder)]
            subprocess.run(git + ["init", "-q"], check=True)
            (folder / ".git/shallow").write_text(repo["commit"] + "\n")
            with pack.open("rb") as source:
                run_logged(git + ["index-pack", "--stdin", "--strict", "-v"], "Unpacking Git objects", stdin=source)
            run_logged(git + ["-c", "core.hooksPath=/dev/null", "-c", "checkout.workers=" + str(min(os.cpu_count() or 4, 16)), "-c", "checkout.thresholdForParallelism=100", "checkout", "--progress", "--detach", repo["commit"]], "Writing source files")
            subprocess.run(git + ["remote", "add", "origin", ORIGIN + "/api/loader/repository/" + repo["name"]], check=True)
            final.parent.mkdir(parents=True, exist_ok=True)
            folder.rename(final)
            (receipts / repo["name"]).write_text(repo["commit"] + " " + repo["sha256"])
    if not (destination / release["workspace"] / "Cargo.toml").is_file():
        raise RuntimeError("Release is missing its Cargo workspace.")
    (destination / (APP + ".json")).write_text(json.dumps(release))
    return destination / release["workspace"]

def private_environment(release):
    rust = ROOT / "toolchain/rust" / (release["rust"] + "-" + TRIPLE)
    output = ROOT / "target"
    env = os.environ.copy()
    for key in ("MAKEPAD_LOADER_EMAIL", "RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        env.pop(key, None)
    env.update(PATH=os.pathsep.join((str(rust / "bin"), str(ROOT / "cargo-home/bin"), env.get("PATH", ""))),
               CARGO_HOME=str(ROOT / "cargo-home"), RUSTUP_HOME=str(ROOT / "rustup-home"),
               RUSTC=str(rust / "bin/rustc"), RUSTDOC=str(rust / "bin/rustdoc"),
               CARGO=str(rust / "bin/cargo"), CARGO_TARGET_DIR=str(output), MAKEPAD_PACKAGE_DIR=".", MAKEPAD_BUILDER_APP=release["id"])
    return env

def build(release):
    if not all(ready_steps(release)):
        raise RuntimeError("Complete compiler and source setup first.")
    rust = ROOT / "toolchain/rust" / (release["rust"] + "-" + TRIPLE)
    source = source_root(release) / release["workspace"]
    output = ROOT / "target"
    env = private_environment(release)
    env.update(CARGO_TERM_COLOR="never", CARGO_TERM_PROGRESS_WHEN="never", MAKEPAD_PACKAGE_DIR=".")
    PROGRESS.package("Build " + release["title"], "Cargo release build", 0, 0)
    artifacts = []
    run_logged([str(rust / "bin/cargo"), "build", "--release", "--message-format=json-render-diagnostics", "-p", release["package"], "--bin", release["binary"]] + (["--locked"] if (source / "Cargo.lock").is_file() else []) + (["--features", ",".join(release["features"])] if release.get("features") else []), "Compiling Rust", artifacts=artifacts, cwd=source, env=env)
    PROGRESS.update("Finishing", "Linking resources to the downloaded source", force=True)
    paths = {}
    for artifact in artifacts:
        target = artifact["target"]
        if target["kind"] == ["custom-build"]:
            continue
        directory = Path(artifact["manifest_path"]).parent.resolve()
        if not directory.is_relative_to(source_root(release)) and not (directory / "resources").is_dir():
            continue
        name = target["name"].replace("-", "_")
        relative = directory.relative_to(ROOT).as_posix()
        if any(c in relative for c in "\t\r\n"):
            raise RuntimeError("Unsupported character in resource path.")
        if name in paths and paths[name] != relative:
            raise RuntimeError("Conflicting resource crates: " + name)
        paths[name] = relative
    if not paths:
        raise RuntimeError("Cargo produced no application resource paths.")
    (ROOT / (release["binary"] + ".bin.makepad-package-paths")).write_text("".join(name + "\t" + path + "\n" for name, path in sorted(paths.items())))
    old_map = ROOT / "makepad-package-paths"
    if old_map.is_file():
        for row in old_map.read_text().splitlines():
            if "\t" in row:
                name, path = row.split("\t", 1)
                paths.setdefault(name, path)
    mapping = ROOT / "makepad-package-paths.next"
    mapping.write_text("".join(name + "\t" + path + "\n" for name, path in sorted(paths.items())))
    mapping.replace(ROOT / "makepad-package-paths")
    # Keep the shell command (scope) and its executable beside one another.
    binary = ROOT / (release["binary"] + ".bin")
    staged = binary.with_suffix(".next")
    shutil.copy2(output / "release" / release["binary"], staged)
    staged.replace(binary)
    PROGRESS.update("Ready", "Build complete", force=True)
    # Relative paths keep an expanded/moved installation portable.
    installed_path().parent.mkdir(exist_ok=True)
    saved = installed_path().with_suffix(".next")
    saved.write_text(json.dumps({"binary": str(binary.relative_to(ROOT)),
                                 "cwd": str(source.relative_to(ROOT)), "release": release}))
    saved.replace(installed_path())
    template = (ROOT / "launcher-template.sh").read_text()
    launcher = ROOT / release["binary"]
    launcher.write_text(template.replace("@SELECTED_APP@", APP))
    launcher.chmod(0o700)

def installed_app():
    saved = installed_path()
    if not saved.is_file():
        raise RuntimeError("Build the app first.")
    installed = json.loads(saved.read_text())
    for key in ("binary", "cwd"):
        path = (ROOT / installed[key]).resolve()
        if not path.is_relative_to(ROOT):
            raise RuntimeError("Installed app escapes its portable folder.")
        installed[key] = path
    return installed

def launch(arguments, project):
    installed = installed_app()
    if arguments and not arguments[0].startswith("-"):
        project = Path(arguments.pop(0)).expanduser().resolve()
    if not project.is_dir():
        raise RuntimeError("Project directory does not exist.")
    env = private_environment(installed["release"]) if "release" in installed else os.environ.copy()
    env.pop("MAKEPAD_LOADER_EMAIL", None)
    command = [str(installed["binary"]), "--cwd", str(project)] + arguments
    # exec preserves terminal ownership and the application's exit status.
    os.chdir(project)
    os.execve(command[0], command, env)

def run():
    project = PROJECT
    if project == ROOT:
        release = installed_app()["release"]
        repo = next((r for r in release["repositories"] if r["name"] == "makepad"), None)
        project = source_root(release) / (repo["path"] if repo else release["workspace"])
    log_path = ROOT / "builder-app.log"
    with log_path.open("w") as log:
        status = subprocess.run([sys.executable, str(ROOT / "makepad-builder.py"), str(ROOT), "--launch"],
                                cwd=project, env=dict(os.environ, MAKEPAD_BUILDER_APP=APP), stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    if status.returncode:
        raise RuntimeError("App exited " + str(status.returncode) + "; see " + str(log_path))

def enable_command():
    leave_screen()
    launcher = ROOT / installed_app()["release"]["binary"]
    folder = Path.home() / ".local/bin"
    link = folder / APP
    print("\nCommand: " + str(link) + " -> " + str(launcher))
    print("Run `" + APP + "` in any project, or `" + APP + " /path/to/project`.")
    if link.is_symlink() and link.resolve() == launcher:
        print("The command is already linked.")
    elif link.exists() or link.is_symlink():
        raise RuntimeError("An existing command occupies " + str(link) + "; it was left unchanged.")
    elif ask("Create this user-local command link?"):
        folder.mkdir(parents=True, exist_ok=True)
        link.symlink_to(launcher)
    else:
        return
    if str(folder) in os.environ.get("PATH", "").split(os.pathsep):
        print("Ready. Your current shell can run " + APP + ".")
        return
    shell = Path(os.environ.get("SHELL", "/bin/sh")).name
    config = {"zsh": Path.home() / ".zshrc", "bash": Path.home() / ".bashrc", "fish": Path.home() / ".config/fish/config.fish"}.get(shell)
    setting = 'fish_add_path --path "$HOME/.local/bin"' if shell == "fish" else 'export PATH="$HOME/.local/bin:$PATH"'
    print("Add this to your shell configuration:\n  " + setting)
    if config and ask("Add the command directory to " + str(config) + "?"):
        config.parent.mkdir(parents=True, exist_ok=True)
        previous = config.read_text() if config.exists() else ""
        if setting not in previous:
            with config.open("a") as output:
                output.write("\n# Makepad application commands (private Rust is not added).\n" + setting + "\n")
        print("Open a new shell, or run the command above in this one.")

def ai_terminal():
    options = [("1", "Claude", "Edit and rebuild this app with the installed claude command"),
               ("2", "Codex", "Edit and rebuild this app with the installed codex command"),
               ("3", "Shell", "Use exit to return to Makepad Builder"),
               ("4", "Other installed tool", "Enter a command and its arguments"), ("q", "Back", "")]
    choice = menu("AI Terminal", ["Project: " + str(PROJECT)], options)
    if choice == "q":
        return
    if choice in ("1", "2"):
        return app_agent("claude" if choice == "1" else "codex")
    command = [os.environ.get("SHELL", "/bin/sh"), "-i"] if choice == "3" else None
    if choice == "4":
        command = shlex.split(input("Command: "))
    if not command:
        return
    available = ROOT / "available" / (APP + ".json")
    saved = installed_path()
    release = json.loads(available.read_text()) if available.is_file() else json.loads(saved.read_text())["release"] if saved.is_file() else None
    env = private_environment(release) if release else os.environ.copy()
    if release:
        env["MAKEPAD_AGENT_CONTEXT"] = str(write_agent_context(release))
    env.pop("MAKEPAD_LOADER_EMAIL", None)
    if shutil.which(command[0], path=env.get("PATH")) is None:
        raise RuntimeError(command[0] + " is not installed or on PATH. No software was installed.")
    leave_screen()
    print("\nProject: " + str(PROJECT) + "\nExit the tool to return to Makepad.\n")
    # Inherit this terminal's stdin/out/err; never open another terminal window.
    subprocess.run(command, cwd=PROJECT, env=env, check=True)

def write_agent_context(release):
    source = source_root(release) / release["workspace"]
    context = ROOT / "agent-context.txt"
    repositories = "\n".join(repo["name"] + " repository: " + str(source_root(release) / repo["path"]) for repo in release["repositories"])
    context.write_text(f"""You are helping customize {release['title']} in a portable compile-on-device installation.
Installation root for this session: {ROOT}
App Cargo workspace: {source}
{repositories}
Cargo package: {release['package']}. Binary: {release['binary']}. Pinned Rust: {release['rust']}.
Read existing AGENTS.md and current widget/Splash examples before editing. Preserve local edits in both shallow repositories. Do not fetch a fresh release or reset a repository to rebuild.
This shell already has the private Rust, Cargo home and build paths. Do not install a compiler, change global PATH, or use rustup. Run cargo check -p {release['package']} from the app workspace and relevant existing tests.
Rebuild and publish edited sources with: {shlex.join([sys.executable, str(ROOT / 'makepad-builder.py'), str(ROOT), '--rebuild'])}
This runs cargo build --release (reusing Cargo.lock when present) with MAKEPAD_PACKAGE_DIR=., updates makepad-package-paths and puts {release['binary']}.bin beside the {APP} shell command. It never downloads or overwrites source edits.
Fonts, SVGs, icons, images and all crate resources remain in the downloaded source. The resource map contains package and target names mapped to paths relative to the executable. Never hardcode the installation root or copy resources elsewhere.
Launch {shlex.quote(str(ROOT / APP))} PROJECT_DIRECTORY --remote to verify on the native GPU. Keep project/state, gracefully close only your own app before replacing its executable, and use app remote captures and quit routes. Do not use simulated GPU or display screenshots.
The installation folder can move. Reopen the agent through Makepad Builder after moving to refresh paths. Do not read or expose makepad-builder.json, email credentials, unrelated user files or authentication storage.
""")
    return context

def app_agent(name):
    saved = installed_path()
    available = ROOT / "available" / (APP + ".json")
    release = json.loads(saved.read_text())["release"] if saved.is_file() else json.loads((available if available.is_file() else ROOT / "latest.json").read_text())
    if not all(ready_steps(release)):
        raise RuntimeError("Complete compiler and source setup before opening an app agent.")
    source = source_root(release) / release["workspace"]
    env = private_environment(release)
    if shutil.which(name, path=env.get("PATH")) is None:
        raise RuntimeError(name + " is not installed or on PATH. No software was installed.")
    context = write_agent_context(release)
    directive = "Before editing read the installation instructions in the file named by the MAKEPAD_AGENT_CONTEXT environment variable. Follow those paths and rebuild instructions for this portable Makepad application. Preserve the existing repository instructions and wait for the user to choose a change."
    command = [name, "-c", "developer_instructions=" + json.dumps(directive)] if name == "codex" else [name, "--append-system-prompt", directive]
    env["MAKEPAD_AGENT_CONTEXT"] = str(context)
    leave_screen()
    subprocess.run(command, cwd=source, env=env, check=True)

def project_menu():
    global PROJECT
    choice = menu("Command / project", ["Project: " + str(PROJECT)], [
        ("d", "Project directory", "Choose the folder opened by the app and agents"),
        ("p", "Enable " + APP + " command", "Optional user-local PATH setup; no global Rust changes"), ("q", "Back", "")])
    if choice == "p":
        enable_command()
    elif choice == "d":
        leave_screen()
        value = input("Project directory (empty keeps current): ").strip()
        if value:
            path = Path(value).expanduser().resolve()
            if not path.is_dir():
                raise RuntimeError("Choose an existing directory.")
            PROJECT = path

def safe_text(text):
    return "".join(c for c in str(text) if c.isprintable())

def pad(text, width):
    text = safe_text(text)
    return (text if len(text) <= width else text[:max(0, width - 1)] + "…").ljust(width)

@contextmanager
def screen():
    if not COLOR:
        yield
        return
    fd = sys.stdin.fileno()
    before = termios.tcgetattr(fd)
    raw = termios.tcgetattr(fd)
    raw[3] &= ~(termios.ICANON | termios.ECHO)
    raw[6][termios.VMIN], raw[6][termios.VTIME] = 0, 2
    try:
        termios.tcsetattr(fd, termios.TCSANOW, raw)
        yield
    finally:
        termios.tcsetattr(fd, termios.TCSANOW, before)

def key():
    char = os.read(sys.stdin.fileno(), 1)
    if char == b"\x1b":
        sequence = b""
        while len(sequence) < 6 and select.select([sys.stdin], [], [], .03)[0]:
            part = os.read(sys.stdin.fileno(), 1)
            if not part:
                break
            sequence += part
            if part.isalpha() or part == b"~":
                break
        return {b"[A": "up", b"OA": "up", b"[B": "down", b"OB": "down", b"[C": "right", b"OC": "right", b"[D": "left", b"OD": "left", b"[5~": "pageup", b"[6~": "pagedown", b"[21~": "q", b"": "q"}.get(sequence, "")
    return "enter" if char in (b"\r", b"\n") else "right" if char == b"\t" else char.decode(errors="ignore").lower()

ACTIVITY = []
def log(text):
    for value in str(text).splitlines():
        value = safe_text(value)
        if value and (not ACTIVITY or ACTIVITY[-1] != value):
            ACTIVITY.append(value)
    del ACTIVITY[:-160]

FRAME = None
SCREEN_ACTIVE = False

def leave_screen():
    global FRAME, SCREEN_ACTIVE
    if SCREEN_ACTIVE:
        print("\033[0m\033[?25h\033[?1049l", end="", flush=True)
    FRAME, SCREEN_ACTIVE = None, False

def present(output):
    global FRAME, SCREEN_ACTIVE
    cols, rows = shutil.get_terminal_size()
    cells = [[(" ", "37;44") for _ in range(cols)] for _ in range(rows)]
    for y, x, text, style, width in output:
        if not 1 <= y <= rows or not 1 <= x <= cols:
            continue
        for i, char in enumerate(pad(text, width)[:min(width, cols - x + 1)]):
            cells[y - 1][x - 1 + i] = (char, style)
    batch = []
    if not SCREEN_ACTIVE:
        batch.append("\033[?1049h")
        SCREEN_ACTIVE = True
        FRAME = None
    batch.append("\033[?2026h\033[?25l\033[?7l")
    for y, line in enumerate(cells):
        if FRAME and len(FRAME) == rows and FRAME[y] == line:
            continue
        batch.append("\033[%d;1H" % (y + 1))
        previous = None
        for char, style in line:
            if style != previous:
                batch.append(ansi(style))
                previous = style
            batch.append(char)
    batch.append("\033[?7h\033[?2026l")
    sys.stdout.write("".join(batch))
    sys.stdout.flush()
    FRAME = cells

def row(output, y, x, text, style, width):
    output.append((y, x, text, style, width))

def panel(output, x, y, width, height, blue=False, shadow=True):
    style = "37;44" if blue else "30;47"
    if shadow:
        for offset in range(1, height + 1):
            row(output, y + offset, x + 1, "", "shadow", width)
    row(output, y, x, "┌" + "─" * (width - 2) + "┐", style, width)
    for offset in range(1, height - 1):
        row(output, y + offset, x, "│" + " " * (width - 2) + "│", style, width)
    row(output, y + height - 1, x, "└" + "─" * (width - 2) + "┘", style, width)

def backdrop(title, footer, height=11):
    cols, rows = shutil.get_terminal_size()
    width = max(24, min(86, cols - 8))
    x, y = (cols - width) // 2 + 1, 5
    output = []
    tw = min(width, len(title) + 6)
    tx = (cols - tw) // 2 + 1
    panel(output, tx, 1, tw, 3)
    row(output, 2, tx + 2, title, "1;30;47", tw - 4)
    panel(output, x, y, width, height)
    log_y = y + height + 1
    if rows > log_y + 4:
        h = rows - log_y - 1
        panel(output, 3, log_y, cols - 6, h, True)
        row(output, log_y, 5, " Activity ", "1;97;44", 10)
        for i, text in enumerate(ACTIVITY[-max(1, h - 2):]):
            row(output, log_y + 1 + i, 5, text, "37;44", cols - 10)
    row(output, rows, 1, footer, "30;47", cols)
    return output, x, y, width

def draw_meter(output, x, y, width, fraction, tick=0):
    row(output, y, x, "[" + "·" * (width - 2) + "]", "90;47", width)
    if fraction is not None:
        fill = round(max(0, min(1, fraction)) * (width - 2))
        row(output, y, x + 1, "█" * fill, "32;47", fill)
    else:
        row(output, y, x + 1 + tick % max(1, width - 5), "███", "90;47", 3)

class ProgressView:
    def __init__(self):
        self.group, self.name, self.index, self.count = "", "", 0, 0
        self.phase, self.detail, self.started, self.last_draw = "", "", time.monotonic(), 0
    def package(self, group, name, index, count):
        self.group, self.name, self.index, self.count = group, name, index, count
        self.phase = ""
        self.update("Preparing", name)
    def update(self, phase, detail, done=0, total=0, unit="", force=False):
        now = time.monotonic()
        changed = phase != self.phase
        if changed:
            self.phase, self.started = phase, now
        if changed or detail != self.detail:
            log(phase + ": " + detail)
            self.detail = detail
        if not force and not changed and now - self.last_draw < .1 and not (total and done >= total):
            return
        self.last_draw = now
        if not COLOR:
            print(phase + ": " + safe_text(detail), flush=True)
            return
        if min(shutil.get_terminal_size()) < 24:
            return
        output, x, y, width = backdrop("Makepad Builder", " Setup is running. The checklist returns when this step finishes.")
        row(output, y + 1, x + 2, self.group or phase, "1;30;47", width - 4)
        row(output, y + 2, x + 2, self.name, "30;47", width - 4)
        if self.count:
            row(output, y + 3, x + 2, "Package %d of %d" % (self.index, self.count), "30;47", width - 4)
            draw_meter(output, x + 2, y + 4, width - 4, 1 if phase == "Ready" else (self.index - 1) / self.count)
        row(output, y + 5, x + 2, phase, "1;30;47", width - 4)
        row(output, y + 6, x + 2, detail, "30;47", width - 4)
        fraction = done / total if total else 1 if phase == "Ready" else None
        elapsed = now - self.started
        draw_meter(output, x + 2, y + 7, width - 4, fraction, int(elapsed * 8))
        amount = ("%.1f / %.1f MiB" % (done / 1048576, total / 1048576) if total else "%.1f MiB" % (done / 1048576)) if unit == "bytes" else (str(done) + (" / " + str(total) if total else "") + " " + unit if unit else "Complete" if phase == "Ready" else "Working…")
        if phase == "Download" and elapsed > .25 and done:
            rate = done / elapsed
            amount += "  %.1f MiB/s" % (rate / 1048576)
            if total > done:
                amount += "  ~%ds left" % ((total - done) / rate + 1)
        row(output, y + 8, x + 2, ("%3.0f%%  " % (fraction * 100) if fraction is not None else "") + amount, "30;47", width - 4)
        present(output)

PROGRESS = ProgressView()
def run_logged(command, phase, artifacts=None, **kwargs):
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, **kwargs)
    pending = b""
    last = "Starting " + Path(str(command[0])).name
    shown_phase, done, total, unit = phase, 0, 0, ""
    try:
        while True:
            readable, _, _ = select.select([process.stdout], [], [], .1)
            if readable:
                chunk = os.read(process.stdout.fileno(), 16384)
                if not chunk:
                    break
                pending += chunk
                while b"\n" in pending or b"\r" in pending:
                    end = min(i for i in (pending.find(b"\n"), pending.find(b"\r")) if i >= 0)
                    value, pending = pending[:end], pending[end + 1:]
                    value = value.decode(errors="replace").strip()
                    if artifacts is not None and value.startswith("{"):
                        try:
                            message = json.loads(value)
                            if message.get("reason") == "compiler-artifact":
                                artifacts.append(message)
                            if "reason" in message:
                                continue
                        except ValueError:
                            pass
                    if EMAIL:
                        value = value.replace(EMAIL, "[email]")
                    if not value:
                        continue
                    progress = re.search(r"^(.+?):\s+\d+%\s+\((\d+)/(\d+)\)", value)
                    if progress:
                        shown_phase = progress[1]
                        last, done, total = "Repository content", int(progress[2]), int(progress[3])
                        unit = "files" if phase == "Writing source files" else "objects"
                    else:
                        last = value
                    PROGRESS.update(shown_phase, last, done, total, unit)
                pending = pending[-16384:]
            PROGRESS.update(shown_phase, last, done, total, unit)
        if pending:
            PROGRESS.update(phase, pending.decode(errors="replace").replace(EMAIL, "[email]") if EMAIL else pending.decode(errors="replace"), force=True)
        if process.wait():
            raise RuntimeError(phase + " failed; see the activity log.")
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait()
        process.stdout.close()

def menu(title, info, options, ready=None, disabled=()):
    def enabled(index):
        return options[index][0] not in disabled
    if not COLOR:
        print("\n" + title + "\n" + "\n".join(info))
        for i, (shortcut, label, _) in enumerate(options):
            print(shortcut + ". " + label + ("" if enabled(i) else " (not ready)"))
        choice = input("Choose: ").strip().lower()
        return choice if all(enabled(i) for i, option in enumerate(options) if option[0] == choice) else ""
    selected = next((i for i, ok in enumerate(ready) if not ok), 2) if ready is not None else 0
    with screen():
        last_frame = None
        offset = 0
        max_offset = 0
        while True:
            cols, rows = shutil.get_terminal_size()
            frame = (cols, rows, selected, offset)
            if frame != last_frame:
                last_frame = frame
                if cols < 48 or rows < 24:
                    print(ansi("37;44") + "\033[2J\033[HResize to at least 48 × 24. Esc closes setup.", end="", flush=True)
                else:
                    intro_rows = 2 if ready is not None else 0
                    inset = 4 if ready is not None else 0
                    w = min(82, cols - 12) - inset
                    lines = [line for value in info for line in (textwrap.wrap(safe_text(value), w, break_on_hyphens=False) or [""])]
                    available = max(1, rows - len(options) - 13 - intro_rows)
                    max_offset = max(0, len(lines) - available)
                    offset = min(offset, max_offset)
                    lines = lines[offset:offset + available]
                    height = max(11, len(lines) + len(options) + 5 + intro_rows)
                    footer = " ↑↓ Select   Enter Continue   Esc Quit" if ready is not None else " ↑↓ Select   Enter Confirm   Esc Back"
                    if max_offset:
                        footer = " PgUp/PgDn Read   ↑↓ Select   Enter Confirm   Esc Back"
                    output, x, y, width = backdrop(title, footer, height)
                    if ready is not None:
                        panel(output, x + 2, y + 1, width - 4, len(lines) + 2, shadow=False)
                    for i, text in enumerate(lines):
                        row(output, y + 1 + i + intro_rows // 2, x + 2 + intro_rows, text, "30;47", width - 4 - inset)
                    first = y + len(lines) + 2 + intro_rows
                    for i, (shortcut, label, _) in enumerate(options):
                        style = ("1;97;40" if enabled(i) else "90;40") if i == selected else "30;47" if enabled(i) else "90;47"
                        mark = ("[✓]" if ready[i] else "[ ]") if ready is not None and i < len(ready) else "   " if ready is not None else ""
                        row(output, first + i, x + 2, mark + " " + shortcut + ". " + label, style, width - 4)
                        if ready is not None and i < len(ready) and ready[i]:
                            row(output, first + i, x + 2, "[✓]", "32;40" if i == selected else "32;47", 3)
                    row(output, y + height - 2, x + 2, options[selected][2], "90;47", width - 4)
                    present(output)
            choice = key()
            if choice == "pageup":
                offset = max(0, offset - 3)
            elif choice == "pagedown":
                offset = min(max_offset, offset + 3)
            elif choice in ("up", "left"):
                selected = (selected - 1) % len(options)
            elif choice in ("down", "right"):
                selected = (selected + 1) % len(options)
            elif choice == "enter" and enabled(selected):
                return options[selected][0]
            elif choice == "q":
                return choice
            elif choice in [o[0] for o in options]:
                selected = next(i for i, o in enumerate(options) if o[0] == choice)
                if enabled(selected):
                    return choice

def devtools_ready():
    if SYSTEM == "Darwin":
        return subprocess.run(["sh", str(ROOT / "check-tools.sh"), "--check"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    if not all(shutil.which(name) for name in ("cc", "c++", "git", "cmake", "pkg-config")):
        return False
    return subprocess.run(["pkg-config", "--exists", "openssl", "x11", "xcursor", "xkbcommon", "xrandr", "xi", "xinerama", "alsa", "libpulse", "wayland-client", "egl", "gl", "glx", "libdrm", "gbm"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0

def ready_steps(release):
    rust = ROOT / "toolchain/rust" / (release["rust"] + "-" + TRIPLE) if release else ROOT / ".missing-toolchain"
    stamp = rust / ".toolchain-version"
    source = source_root(release) if release else ROOT / ".missing-source"
    return [devtools_ready(), bool(release and stamp.is_file() and stamp.read_text() == release["rust"] + " " + TRIPLE and all((rust / ("bin/" + name)).is_file() for name in ("cargo", "rustc"))),
            bool(release and all(repo_ready(release, r) for r in release["repositories"]) and (source / release["workspace"] / "Cargo.toml").is_file())]

def install_compiler(release):
    ready = ready_steps(release)
    if all(ready[:2]):
        log("Compiler is already installed.")
        return
    packages = dependency_packages() if SYSTEM == "Linux" and not ready[0] else None
    info = ["Compiler tools and Rust " + release["rust"],
            "Rust: MIT and Apache 2.0 license notices",
            "https://www.rust-lang.org/policies/licenses"]
    if SYSTEM == "Darwin":
        info.append("Apple developer tools must already be installed and licensed.")
    elif packages:
        info.extend(["Development packages for " + packages[0] + ":", shlex.join(packages[1]),
                     "These system packages use their respective licenses."])
    info.append("Rust will be installed privately in this folder.")
    if not ask("Accept these terms and prepare the compiler?", info, "Compiler license terms"):
        log("Compiler installation cancelled.")
        return
    if not ready[0]:
        leave_screen()
        if SYSTEM == "Darwin":
            subprocess.run(["sh", str(ROOT / "check-tools.sh")], check=True)
        else:
            subprocess.run(packages[1], check=True)
        if not devtools_ready():
            raise RuntimeError("Developer tools are not ready yet. Complete their setup, then retry.")
    if not ready[1]:
        toolchain(release)
    PROGRESS.update("Ready", "Compiler installed and verified", force=True)

def app_menu(app):
    global APP
    previous = APP
    available = ROOT / "available"
    cached = available / (app + ".json")
    current = available / (previous + ".json")
    if not cached.is_file() and current.is_file():
        release = json.loads(current.read_text())
        repositories = [repo for repo in release["repositories"] if repo["name"] == "makepad"]
        if repositories:
            release.update(next(entry for entry in APPS if entry["id"] == app))
            release["repositories"] = repositories
            release["public"] = True
            cached.write_text(json.dumps(release))
    APP = app
    try:
        main(primary=False)
    finally:
        APP = previous
        (ROOT / "selected-app").write_text(APP)

def other_apps():
    apps = [app for app in APPS if app.get("menu") == "other"]
    page = 0
    while True:
        entries = apps[page * 8:page * 8 + 8]
        options = [(str(i + 1), app["title"], "Public Makepad app") for i, app in enumerate(entries)]
        if (page + 1) * 8 < len(apps):
            options.append(("9", "Next page", ""))
        if page:
            options.append(("0", "Previous page", ""))
        options.append(("q", "Back", ""))
        result = menu("Other Apps", ["Page %d of %d · shared compiler, source and build cache" % (page + 1, (len(apps) + 7) // 8)], options)
        if result == "q":
            return
        if result in ("0", "9"):
            page += 1 if result == "9" else -1
            continue
        app_menu(entries[int(result) - 1]["id"])

def main(primary=True):
    global APP
    if primary:
        APP = "scope"
    (ROOT / "selected-app").write_text(APP)
    available = ROOT / "available"
    available.mkdir(exist_ok=True)
    saved = available / (APP + ".json")
    cached = saved if saved.is_file() else ROOT / "latest.json"
    release = json.loads(cached.read_text()) if cached.is_file() else None
    if release and release["id"] != APP:
        release = None
    def save_release(value):
        saved.write_text(json.dumps(value))
        (ROOT / "latest.json").write_text(json.dumps(value))
    while True:
        ready = ready_steps(release)
        title = release["title"] if release else next(app["title"] for app in APPS if app["id"] == APP)
        def label(value):
            return value.replace("{app}", title).replace("{tools}", "Apple developer tools, SDK and Git" if SYSTEM == "Darwin" else "Distro development packages")
        options = []
        for entry in MENU:
            key, text, detail = entry.split("|")
            if not primary and key in ("4", "5"):
                continue
            if not primary and key == "6":
                key = "4"
            options.append((key, label(text), label(detail)))
        if not primary:
            options.append(("q", "Back to Builder" if APP == "wm" else "Back to Other Apps", ""))
        disabled = () if all(ready) else ("3", "6" if primary else "4")
        heading = "Makepad Builder" if primary else "Makepad Builder · " + title
        choice = menu(heading, [ABOUT.replace("{app}", title)], options, [all(ready[:2]), ready[2]], disabled)
        if choice == "q":
            break
        try:
            if choice == "1":
                if not release:
                    release = latest()
                    save_release(release)
                install_compiler(release)
            elif choice == "2":
                release = latest()
                save_release(release)
                checkout(release)
                PROGRESS.update("Ready", "Source verified and checked out", force=True)
            elif choice == "3":
                if not all(ready_steps(release)):
                    raise RuntimeError("Complete compiler and source setup first.")
                build(release)
                run()
            elif primary and choice == "4":
                other_apps()
            elif primary and choice == "5":
                app_menu("wm")
            elif choice == ("6" if primary else "4"):
                ai_terminal()
        except (RuntimeError, OSError, ValueError, KeyError, IndexError, AttributeError, subprocess.CalledProcessError) as error:
            message = safe_text(str(error).replace(EMAIL, "[email]") if EMAIL else str(error))
            log(message)
            menu("Step could not complete", [message], [("↵", "Continue", "Enter returns to setup")])

if __name__ == "__main__":
    try:
        if len(sys.argv) > 2 and sys.argv[2] == "--launch":
            launch(sys.argv[3:], Path.cwd())
        elif len(sys.argv) > 2 and sys.argv[2] == "--rebuild":
            installed = installed_path()
            release = json.loads(installed.read_text())["release"] if installed.is_file() else json.loads((ROOT / "latest.json").read_text())
            build(release)
        else:
            main()
    except (KeyboardInterrupt, EOFError):
        print("\nMakepad Builder closed.")
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        print(safe_text(str(error).replace(EMAIL, "[email]") if EMAIL else str(error)), file=sys.stderr)
        sys.exit(1)
    finally:
        if COLOR:
            leave_screen()
