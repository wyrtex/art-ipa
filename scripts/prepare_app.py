#!/usr/bin/env python3
"""Prepare a Craft app checkout for an iOS build.

Usage: prepare_app.py <checkout-dir> <app> <art-ipa-root>

1. Points `rfd` and `arboard` at the iOS-capable copies in `shims/` ([patch.crates-io]).
2. Excludes iOS from the apps' "desktop Unix that is not macOS" (Linux/BSD) branches:
   `all(unix, not(target_os = "macos"))` becomes `all(unix, not(any(target_os = "macos",
   target_os = "ios")))` in the workspace's Rust sources and manifests (outside `vendor/`).
   Otherwise iOS (also `unix`) pulls in X11/Wayland/glutin code and crates.
3. Adds the `craft-ios` start-up crate to `apps/<app>` and calls `craft_ios::init()` first thing
   in its `main`.
4. Applies `patches/<app>/*.patch` (if any), in name order.

Idempotent: running it twice on the same checkout changes nothing the second time.
"""
import glob
import os
import re
import subprocess
import sys

MARKER = "// art-ipa: iOS start-up"


def main() -> None:
    checkout, app, root = sys.argv[1:]
    root = os.path.abspath(root)
    shims = os.path.join(root, "shims")
    patch_workspace(checkout, shims, ["rfd", "arboard"])
    exclude_ios_from_linux_cfgs(checkout)
    add_ios_dependency(os.path.join(checkout, "apps", app, "Cargo.toml"), os.path.join(shims, "craft-ios"))
    call_init(os.path.join(checkout, "apps", app, "src", "main.rs"))
    for patch in sorted(glob.glob(os.path.join(root, "patches", app, "*.patch"))):
        if subprocess.run(["git", "apply", "--reverse", "--check", patch], cwd=checkout, capture_output=True).returncode == 0:
            continue  # already applied
        subprocess.run(["git", "apply", "--whitespace=nowarn", patch], cwd=checkout, check=True)
        print(f"applied {os.path.relpath(patch, root)}")


def patch_workspace(checkout: str, shims: str, crates: list) -> None:
    path = os.path.join(checkout, "Cargo.toml")
    text = read(path)
    m = re.search(r"^\[patch\.crates-io\]\s*\n(.*?)(?=^\[|\Z)", text, re.M | re.S)
    existing = m.group(1) if m else ""
    lines = []
    for c in crates:
        if re.search(rf"^\s*{re.escape(c)}\s*=", existing, re.M):
            if os.path.join(shims, c) in existing:
                continue
            sys.exit(f"{path}: [patch.crates-io] already patches {c}; merge the iOS changes into that copy")
        lines.append(f'{c} = {{ path = "{os.path.join(shims, c)}" }}')
    if not lines:
        return
    if m is None:
        text = text.rstrip("\n") + "\n\n[patch.crates-io]\n" + "\n".join(lines) + "\n"
    else:
        text = text[: m.start(1)] + "\n".join(lines) + "\n" + text[m.start(1):]
    write(path, text)


CFG_REWRITES = [
    ('unix, not(target_os = "macos")', 'unix, not(any(target_os = "macos", target_os = "ios"))'),
    ('unix, not(target_os="macos")', 'unix, not(any(target_os="macos", target_os="ios"))'),
    ('unix, not(any(target_os = "macos", target_os = "android"', 'unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"'),
    ('unix, any(not(target_os = "macos"), test)', 'unix, any(not(any(target_os = "macos", target_os = "ios")), test)'),
]


def exclude_ios_from_linux_cfgs(checkout: str) -> None:
    changed = 0
    for dirpath, dirnames, filenames in os.walk(checkout):
        dirnames[:] = [d for d in dirnames if d not in ("target", "vendor", ".git", "node_modules")]
        for name in filenames:
            if not (name.endswith(".rs") or name == "Cargo.toml"):
                continue
            path = os.path.join(dirpath, name)
            text = read(path)
            new = text
            for old, repl in CFG_REWRITES:
                new = new.replace(old, repl)
            if new != text:
                write(path, new)
                changed += 1
    if changed:
        print(f"excluded iOS from Linux-only cfgs in {changed} files")


def add_ios_dependency(manifest: str, crate_dir: str) -> None:
    text = read(manifest)
    if "craft-ios" in text:
        return
    dep = f'craft-ios = {{ path = "{crate_dir}" }}'
    header = re.search(r"""^\[target\.'cfg\(target_os\s*=\s*"ios"\)'\.dependencies\]\s*\n""", text, re.M)
    if header:
        text = text[: header.end()] + dep + "\n" + text[header.end():]
    else:
        text = text.rstrip("\n") + "\n\n[target.'cfg(target_os = \"ios\")'.dependencies]\n" + dep + "\n"
    write(manifest, text)


def call_init(main_rs: str) -> None:
    text = read(main_rs)
    if MARKER in text:
        return
    m = re.search(r"^fn main\(\)[^{]*\{[ \t]*\n", text, re.M)
    if not m:
        sys.exit(f"{main_rs}: no `fn main()` found")
    text = text[: m.end()] + f"    {MARKER}\n    #[cfg(target_os = \"ios\")]\n    craft_ios::init();\n" + text[m.end():]
    write(main_rs, text)


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def write(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


if __name__ == "__main__":
    main()
