"""Tests for the bundled-LSP receipt generator/verifier. Run directly:
python3 scripts/tests/test_lsp_receipt.py

The receipt is what crates/termlab_tauri/src/lsp/catalog.rs checks before it
launches a bundled server, so the failure modes below are the ones that decide
whether a tampered resource tree is caught or executed.
"""

import hashlib
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lsp"))
import importlib

receipt = importlib.import_module("receipt")

PINS = [
    "--artifact",
    "node=24.19.0",
    "--artifact",
    "typescript-language-server=6.0.0",
    "--artifact",
    "typescript=6.0.3",
    "--artifact",
    "rust-analyzer=2026-08-24",
]

failures = []


def check(condition, description):
    if condition:
        print(f"  ok   {description}")
    else:
        print(f"  FAIL {description}")
        failures.append(description)


def build_tree(root):
    """A miniature stand-in for packaging/lsp/dist."""
    arch_root = os.path.join(root, "arm64")
    for relative, contents in (
        ("node/bin/node", b"#!/bin/sh\nexit 0\n"),
        ("rust-analyzer/rust-analyzer", b"#!/bin/sh\nexit 0\n"),
        ("typescript/node_modules/typescript-language-server/lib/cli.mjs", b"export {};\n"),
    ):
        full = os.path.join(arch_root, relative)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "wb") as handle:
            handle.write(contents)
    return arch_root


def run(argv):
    """Run receipt.main, turning its SystemExit refusals into an exit code."""
    try:
        return receipt.main(argv)
    except SystemExit as error:
        if isinstance(error.code, str):
            print(f"       (refused: {error.code})")
            return 1
        return error.code or 1


def generate(arch_root, output):
    return run(["generate", "--arch-root", arch_root, "--output", output] + PINS)


def verify(arch_root, output):
    return run(["verify", "--arch-root", arch_root, "--receipt", output] + PINS)


def main():
    with tempfile.TemporaryDirectory() as root:
        arch_root = build_tree(root)
        output = os.path.join(root, "manifest.json")

        check(generate(arch_root, output) == 0, "generate writes a receipt for a staged tree")
        with open(output, encoding="utf-8") as handle:
            document = json.load(handle)

        check(document["schema"] == 1, "receipt declares schema 1")
        check(document["platform"] == "macos", "receipt declares the macos platform")
        check(document["architecture"] == "arm64", "receipt declares the arm64 architecture")
        check(len(document["artifacts"]) == 4, "receipt records all four pinned artifacts")
        check(
            [entry["relativePath"] for entry in document["files"]]
            == sorted(entry["relativePath"] for entry in document["files"]),
            "recorded paths are sorted, so two fetches produce the same bytes",
        )
        node_entry = next(
            entry for entry in document["files"] if entry["relativePath"] == "node/bin/node"
        )
        check(
            node_entry["sha256"]
            == hashlib.sha256(open(os.path.join(arch_root, "node/bin/node"), "rb").read()).hexdigest(),
            "recorded digest is the file's real SHA-256",
        )

        check(verify(arch_root, output) == 0, "verify accepts the tree it just recorded")

        # Tampering with a recorded file must be caught by digest, not by size.
        target = os.path.join(arch_root, "rust-analyzer/rust-analyzer")
        original = open(target, "rb").read()
        with open(target, "wb") as handle:
            handle.write(b"#!/bin/sh\nexit 1\n")
        check(
            os.path.getsize(target) == len(original),
            "the tampered file is the same size as the original",
        )
        check(verify(arch_root, output) == 1, "verify rejects a same-size content change")
        with open(target, "wb") as handle:
            handle.write(original)
        check(verify(arch_root, output) == 0, "verify accepts the restored tree")

        # A file added after the receipt was written must not pass unnoticed.
        smuggled = os.path.join(arch_root, "node/bin/extra")
        with open(smuggled, "wb") as handle:
            handle.write(b"payload\n")
        check(verify(arch_root, output) == 1, "verify rejects a file absent from the receipt")
        os.remove(smuggled)

        # A removed file must fail rather than be skipped.
        removed = os.path.join(
            arch_root, "typescript/node_modules/typescript-language-server/lib/cli.mjs"
        )
        contents = open(removed, "rb").read()
        os.remove(removed)
        check(verify(arch_root, output) == 1, "verify rejects a missing recorded file")
        with open(removed, "wb") as handle:
            handle.write(contents)

        # Pins that disagree with packaging/lsp/manifest.toml must not verify.
        wrong_pins = ["verify", "--arch-root", arch_root, "--receipt", output] + [
            "--artifact",
            "node=24.19.1",
            "--artifact",
            "typescript-language-server=6.0.0",
            "--artifact",
            "typescript=6.0.3",
            "--artifact",
            "rust-analyzer=2026-08-24",
        ]
        check(run(wrong_pins) == 1, "verify rejects a receipt pinned to other versions")

        # Symlinks would let the receipt describe a file the runtime never opens.
        link = os.path.join(arch_root, "node/bin/node-link")
        os.symlink("node", link)
        check(
            generate(arch_root, os.path.join(root, "other.json")) != 0,
            "generate refuses a tree containing symlinks",
        )
        os.remove(link)

        # An empty file can never satisfy the runtime, so it must not be recorded.
        empty = os.path.join(arch_root, "node/bin/empty")
        Path(empty).touch()
        check(
            generate(arch_root, os.path.join(root, "empty.json")) != 0,
            "generate refuses to record an empty file",
        )
        os.remove(empty)

        missing_root = os.path.join(root, "gone")
        os.makedirs(missing_root)
        shutil.rmtree(missing_root)
        check(
            run(["verify", "--arch-root", missing_root] + PINS) == 1,
            "verify rejects a missing architecture root",
        )

    if failures:
        print(f"\n{len(failures)} check(s) failed")
        return 1
    print("\nlsp receipt: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
