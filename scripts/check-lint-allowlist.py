#!/usr/bin/env python3
"""Reject production lint suppressions that are not on the allowlist.

The allowlist lives in the workspace Cargo.toml comment ("Production allowlist").
Everything after the first `#[cfg(test)]` line of a src file is test code and is skipped;
tests/ and benches/ are not scanned at all.
"""
import pathlib
import re
import sys

ALLOWED = re.compile(
    r"clippy::(arithmetic_side_effects|indexing_slicing|as_conversions|cast_\w+)"
)
ATTR = re.compile(r"#!?\[\s*(allow|expect)\s*\((.*?)\)\s*\]", re.DOTALL)

failures = []
for path in sorted(pathlib.Path("crates").glob("*/src/**/*.rs")):
    text = path.read_text()
    marker = text.find("#[cfg(test)]")
    prod = text if marker == -1 else text[:marker]
    for match in ATTR.finditer(prod):
        line = prod.count("\n", 0, match.start()) + 1
        if match.group(0).startswith("#!"):
            failures.append(f"{path}:{line}: module-wide suppression is forbidden")
            continue
        body = re.split(r"reason\s*=", match.group(2))[0]
        for lint in re.findall(r"[A-Za-z_]+(?:::[A-Za-z_]+)+|[a-z_]+", body):
            if not ALLOWED.fullmatch(lint):
                failures.append(f"{path}:{line}: `{lint}` is not on the production allowlist")

if failures:
    print("\n".join(failures))
    sys.exit(1)
print("lint allowlist check passed")
