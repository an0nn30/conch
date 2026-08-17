#!/usr/bin/env python3
"""Set the macOS bundle's two version keys.

CFBundleShortVersionString is the user-visible marketing version and must be at
most three period-separated non-negative integers — Apple rejects a pre-release
suffix there, so "3.0.0-rc.2" becomes "3.0.0".

CFBundleVersion keeps the full string, including any pre-release suffix, so the
build remains identifiable from the bundle alone.

Editing by key name rather than by value pattern is the point: the previous
sed-on-the-value approach stopped matching the moment the value gained a
suffix, and silently left the plist stale.
"""
import re
import sys

if len(sys.argv) != 3:
    sys.exit("usage: set_bundle_version.py <full-version> <numeric-version>")

full, numeric = sys.argv[1], sys.argv[2]
if not re.fullmatch(r"\d+(\.\d+){0,2}", numeric):
    sys.exit(f"error: {numeric!r} is not a valid CFBundleShortVersionString")

path = "packaging/macos/Info.plist"
with open(path) as fh:
    text = fh.read()


def set_key(text, key, value):
    pattern = re.compile(
        r"(<key>" + re.escape(key) + r"</key>\s*<string>)[^<]*(</string>)"
    )
    new_text, count = pattern.subn(lambda m: m.group(1) + value + m.group(2), text)
    if count != 1:
        sys.exit(f"error: expected exactly one {key} in {path}, found {count}")
    return new_text


text = set_key(text, "CFBundleVersion", full)
text = set_key(text, "CFBundleShortVersionString", numeric)

with open(path, "w") as fh:
    fh.write(text)

print(f"Info.plist: CFBundleVersion={full}, CFBundleShortVersionString={numeric}")
