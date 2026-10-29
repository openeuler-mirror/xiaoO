#!/usr/bin/env python3
"""Reject direct process creation outside the coordination leaf.

All zero-argument spawn/status/output method calls are inspected, including
builder chains and multiline calls. Existing non-process methods have exact
source-line/count exceptions; additional calls require review. UFCS calls to
spawn/status/output are inspected too. This is a conservative syntax gate,
not Rust type inference. Tests and non-workspace cerberus are out of scope.
"""
import collections
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "scripts/baselines/non-process-calls.json"
LEAF = "crates/process-coordination/src/lib.rs"
CALL = re.compile(r"\.\s*(?:spawn|status|output)\s*\(\s*\)|::\s*(?:spawn|status|output)\s*\(")
RAW = re.compile(r'(?:br|cr|r)(#*)"')
def mask_rust(source):
    """Mask Rust literals/comments while preserving source offsets and lines."""
    masked = list(source)
    size = len(source)
    def erase(first, last):
        for index in range(first, last):
            if source[index] not in "\r\n": masked[index] = " "
    index = 0
    while index < size:
        first = index
        if source.startswith("//", index):
            end = source.find("\n", index)
            index = size if end < 0 else end
        elif source.startswith("/*", index):
            index += 2
            depth = 1
            while index < size and depth:
                if source.startswith("/*", index): depth += 1; index += 2
                elif source.startswith("*/", index): depth -= 1; index += 2
                else: index += 1
            if depth: raise ValueError("unterminated block comment")
        else:
            raw = None
            if index == 0 or not (source[index - 1].isalnum() or source[index - 1] == "_"):
                raw = RAW.match(source, index)
            if raw:
                closing = '"' + raw[1]
                end = source.find(closing, raw.end())
                if end < 0: raise ValueError("unterminated raw string")
                index = end + len(closing)
            elif source[index] == '"':
                index += 1
                while index < size:
                    if source[index] == "\\": index += 2
                    elif source[index] == '"': index += 1; break
                    else: index += 1
                else: raise ValueError("unterminated string")
            elif source[index] == "'":
                end = index + 1
                if end < size and source[end] == "\\":
                    end += 1
                    if source.startswith("u{", end):
                        brace = source.find("}", end + 2)
                        end = size if brace < 0 else brace + 1
                    elif end < size and source[end] == "x": end += 3
                    else: end += 1
                else: end += 1
                if end < size and source[end] == "'": index = end + 1
                else:
                    # An apostrophe followed by an identifier is a lifetime or
                    # label, not the start of a character or string literal.
                    index += 1
                    continue
            else:
                index += 1
                continue
        erase(first, index)
    return "".join(masked)

def creation_calls(source):
    masked = mask_rust(source)
    result = collections.Counter()
    for match in CALL.finditer(masked):
        begin = masked.rfind("\n", 0, match.start()) + 1
        end = masked.find("\n", match.end())
        if end < 0: end = len(masked)
        line = " ".join(masked[begin:end].split())
        result[line] += 1
    return result

def calls(path):
    return creation_calls(path.read_text())

def main():
    allowed = json.loads(BASELINE.read_text())
    failures = []
    for base in [ROOT / "crates", ROOT / "apps"]:
        for path in sorted(base.rglob("*.rs")):
            relative = path.relative_to(ROOT).as_posix()
            if relative.startswith("crates/cerberus/") or "/src/" not in relative: continue
            try: actual = dict(calls(path))
            except ValueError as error:
                failures.append((relative, str(error)))
                continue
            if relative == LEAF:
                if actual != {"command.spawn()": 3}: failures.append((relative, actual))
            elif actual != allowed.get(relative, {}):
                failures.append((relative, actual))
    if failures:
        for path, actual in failures: print(f"FAIL [PROCESS-CREATION] {path}: unreviewed spawn/status/output call set: {actual}", file=sys.stderr)
        return 1
    print("PASS: process creation is confined to the coordination leaf")
    return 0

if __name__ == "__main__": sys.exit(main())
