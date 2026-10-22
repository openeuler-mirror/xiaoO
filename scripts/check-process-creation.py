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
# Preserve newlines and byte positions while masking strings/comments.
TOKEN = re.compile(r'//[^\n]*|/\*.*?\*/|r(#+)?".*?"\1|"(?:\\.|[^"\\])*"', re.S)

def calls(path):
    source = path.read_text()
    masked = TOKEN.sub(lambda m: "".join("\n" if c == "\n" else " " for c in m.group()), source)
    result = collections.Counter()
    for match in CALL.finditer(masked):
        begin = masked.rfind("\n", 0, match.start()) + 1
        end = masked.find("\n", match.end())
        if end < 0: end = len(masked)
        line = " ".join(masked[begin:end].split())
        result[line] += 1
    return result

def main():
    allowed = json.loads(BASELINE.read_text())
    failures = []
    for base in [ROOT / "crates", ROOT / "apps"]:
        for path in sorted(base.rglob("*.rs")):
            relative = path.relative_to(ROOT).as_posix()
            if relative.startswith("crates/cerberus/") or "/src/" not in relative: continue
            actual = dict(calls(path))
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
