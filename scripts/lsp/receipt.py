#!/usr/bin/env python3
"""Generate and verify the installed LSP resource receipt (``lsp/manifest.json``).

The receipt is the runtime trust anchor. ``crates/termlab_tauri/src/lsp/catalog.rs``
refuses to launch a bundled language server unless every file it needs is named
in the receipt with a matching size and SHA-256, so this module is deliberately
the only writer of that format: the packaging script and the Rust consumer
cannot drift apart if they share one implementation.

Layout (see the design spec's "macOS resources" section):

    <lsp-root>/manifest.json          the receipt this module writes
    <lsp-root>/THIRD_PARTY_NOTICES.md
    <lsp-root>/arm64/...              the architecture root passed as --arch-root

Usage:
    receipt.py generate --arch-root DIR --output FILE --artifact id=version ...
    receipt.py verify   --arch-root DIR --artifact id=version ...
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from typing import Dict, List, Tuple

SCHEMA = 1
PLATFORM = "macos"
ARCHITECTURE = "arm64"
HASH_CHUNK_BYTES = 1024 * 1024
# Mirrors MAX_RESOURCE_FILE_BYTES in catalog.rs; a larger file could never be
# validated at runtime, so refuse to record one.
MAX_RESOURCE_FILE_BYTES = 512 * 1024 * 1024


def sha256_of(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while True:
            chunk = handle.read(HASH_CHUNK_BYTES)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def walk_regular_files(arch_root: str) -> List[str]:
    """Return every regular file below ``arch_root`` as a sorted relative path.

    Symlinks are an error rather than a skip: the fetch script strips them, and
    a surviving one would let the receipt describe a file the runtime does not
    actually open.
    """
    relative_paths: List[str] = []
    symlinks: List[str] = []
    for directory, subdirectories, filenames in os.walk(arch_root, followlinks=False):
        subdirectories.sort()
        for name in sorted(subdirectories):
            full = os.path.join(directory, name)
            if os.path.islink(full):
                symlinks.append(os.path.relpath(full, arch_root))
        for name in sorted(filenames):
            full = os.path.join(directory, name)
            relative = os.path.relpath(full, arch_root)
            if os.path.islink(full):
                symlinks.append(relative)
                continue
            if not os.path.isfile(full):
                continue
            relative_paths.append(relative)
    if symlinks:
        raise SystemExit(
            "receipt: staged tree still contains symlinks: " + ", ".join(sorted(symlinks))
        )
    return sorted(relative_paths)


def parse_artifacts(values: List[str]) -> List[Tuple[str, str]]:
    artifacts: List[Tuple[str, str]] = []
    for value in values:
        identifier, separator, version = value.partition("=")
        if not separator or not identifier or not version:
            raise SystemExit(f"receipt: malformed --artifact value: {value!r}")
        artifacts.append((identifier, version))
    if len(artifacts) != 4:
        raise SystemExit(
            f"receipt: expected 4 pinned artifacts, got {len(artifacts)}"
        )
    return artifacts


def build_receipt(arch_root: str, artifacts: List[Tuple[str, str]]) -> Dict[str, object]:
    files = []
    for relative in walk_regular_files(arch_root):
        full = os.path.join(arch_root, relative)
        size = os.path.getsize(full)
        if size == 0:
            raise SystemExit(f"receipt: refusing to record empty file: {relative}")
        if size > MAX_RESOURCE_FILE_BYTES:
            raise SystemExit(
                f"receipt: file exceeds the runtime size limit: {relative} ({size} bytes)"
            )
        files.append(
            {"relativePath": relative, "sha256": sha256_of(full), "size": size}
        )
    return {
        "schema": SCHEMA,
        "platform": PLATFORM,
        "architecture": ARCHITECTURE,
        "artifacts": [{"id": identifier, "version": version} for identifier, version in artifacts],
        "files": files,
    }


def generate(arguments: argparse.Namespace) -> int:
    receipt = build_receipt(arguments.arch_root, parse_artifacts(arguments.artifact))
    with open(arguments.output, "w", encoding="utf-8") as handle:
        json.dump(receipt, handle, indent=2)
        handle.write("\n")
    print(f"receipt: recorded {len(receipt['files'])} files in {arguments.output}")
    return 0


def verify(arguments: argparse.Namespace) -> int:
    expected_artifacts = parse_artifacts(arguments.artifact)
    receipt_path = arguments.receipt or os.path.join(
        os.path.dirname(os.path.abspath(arguments.arch_root)), "manifest.json"
    )
    problems: List[str] = []

    if not os.path.isfile(receipt_path):
        print(f"receipt: missing receipt: {receipt_path}", file=sys.stderr)
        return 1
    try:
        with open(receipt_path, "r", encoding="utf-8") as handle:
            receipt = json.load(handle)
    except (OSError, ValueError) as error:
        print(f"receipt: unreadable receipt {receipt_path}: {error}", file=sys.stderr)
        return 1

    if receipt.get("schema") != SCHEMA:
        problems.append(f"schema is {receipt.get('schema')!r}, expected {SCHEMA}")
    if receipt.get("platform") != PLATFORM:
        problems.append(f"platform is {receipt.get('platform')!r}, expected {PLATFORM!r}")
    if receipt.get("architecture") != ARCHITECTURE:
        problems.append(
            f"architecture is {receipt.get('architecture')!r}, expected {ARCHITECTURE!r}"
        )

    recorded_artifacts = [
        (entry.get("id"), entry.get("version")) for entry in receipt.get("artifacts", [])
    ]
    if sorted(recorded_artifacts) != sorted(expected_artifacts):
        problems.append(
            "artifact pins do not match packaging/lsp/manifest.toml: "
            f"receipt={sorted(recorded_artifacts)} manifest={sorted(expected_artifacts)}"
        )

    entries = receipt.get("files", [])
    recorded_paths = [entry.get("relativePath") for entry in entries]
    if len(set(recorded_paths)) != len(recorded_paths):
        problems.append("receipt lists a relative path more than once")
    for relative in recorded_paths:
        if not isinstance(relative, str) or not relative:
            problems.append(f"receipt has a non-string relative path: {relative!r}")
        elif os.path.isabs(relative) or ".." in relative.split(os.sep):
            problems.append(f"receipt has an unsafe relative path: {relative}")

    for entry in entries:
        relative = entry.get("relativePath")
        if not isinstance(relative, str) or not relative:
            continue
        full = os.path.join(arguments.arch_root, relative)
        if os.path.islink(full) or not os.path.isfile(full):
            problems.append(f"recorded file is missing: {relative}")
            continue
        size = os.path.getsize(full)
        if size != entry.get("size"):
            problems.append(
                f"size mismatch for {relative}: on disk {size}, receipt {entry.get('size')}"
            )
            continue
        digest = sha256_of(full)
        if digest != entry.get("sha256"):
            problems.append(
                f"sha256 mismatch for {relative}: on disk {digest}, receipt {entry.get('sha256')}"
            )

    on_disk = set(walk_regular_files(arguments.arch_root))
    unrecorded = sorted(on_disk - {path for path in recorded_paths if isinstance(path, str)})
    if unrecorded:
        problems.append(
            "staged files are absent from the receipt: " + ", ".join(unrecorded[:10])
        )

    if problems:
        for problem in problems:
            print(f"receipt: {problem}", file=sys.stderr)
        return 1
    print(f"receipt: verified {len(entries)} files against {receipt_path}")
    return 0


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    generator = subparsers.add_parser("generate", help="write a receipt for a staged tree")
    generator.add_argument("--arch-root", required=True)
    generator.add_argument("--output", required=True)
    generator.add_argument("--artifact", action="append", default=[])
    generator.set_defaults(handler=generate)

    verifier = subparsers.add_parser("verify", help="check a staged tree against its receipt")
    verifier.add_argument("--arch-root", required=True)
    verifier.add_argument("--receipt", default=None)
    verifier.add_argument("--artifact", action="append", default=[])
    verifier.set_defaults(handler=verify)

    arguments = parser.parse_args(argv)
    if not os.path.isdir(arguments.arch_root):
        print(f"receipt: not a directory: {arguments.arch_root}", file=sys.stderr)
        return 1
    return arguments.handler(arguments)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
