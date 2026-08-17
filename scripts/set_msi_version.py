#!/usr/bin/env python3
"""Set the MSI package's ProductVersion in packaging/windows/termlab.wxs.

Targets the <Package> element's own `Version` attribute by name. A regex over
attribute *values* is unsafe here: `Version="..."` also appears as a substring
of `InstallerVersion="200"` (which declares the minimum Windows Installer
version and must stay an integer), so a value-pattern edit silently rewrites
the wrong attribute and leaves the real one stale.

MSI ProductVersion must be numeric — at most three dot-separated integers — so
any pre-release suffix is stripped before it gets here.
"""
import re
import sys

if len(sys.argv) != 2:
    sys.exit("usage: set_msi_version.py <numeric-version>")

version = sys.argv[1]
if not re.fullmatch(r"\d+(\.\d+){0,2}", version):
    sys.exit(f"error: {version!r} is not a valid MSI ProductVersion")

path = "packaging/windows/termlab.wxs"
with open(path) as fh:
    text = fh.read()

# (?<![A-Za-z]) keeps this off InstallerVersion and any other *Version attribute.
pattern = re.compile(r'(?<![A-Za-z])(Version=")[^"]*(")')
new_text, count = pattern.subn(lambda m: m.group(1) + version + m.group(2), text)
if count != 1:
    sys.exit(f"error: expected exactly one Package Version in {path}, found {count}")

with open(path, "w") as fh:
    fh.write(new_text)

print(f"termlab.wxs: Version={version}")
