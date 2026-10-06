#!/usr/bin/env python3
"""agent_moss 的用例 JSON 必须能被解析。

用例文件里的 `prompt` 是一段要交给被测 agent 的文本，常包含 shell 片段。写成
`\\$`（JSON 不允许的转义）时整个文件读不出来 —— `run-deny-cron-exfil.json` 就是
这样，而它在 JSON 源码里保留字面反斜杠的正确写法是 `\\\\$`。

    python3 plugins/tests/hookers/agent_moss/test_cases_parse.py
"""

import json
import pathlib
import sys

CASES = pathlib.Path(__file__).resolve().parent / "cases"


def main() -> int:
    if not CASES.is_dir():
        print("FAIL  找不到用例目录：%s" % CASES, file=sys.stderr)
        return 1
    files = sorted(CASES.glob("*.json"))
    if not files:
        print("FAIL  一个用例文件都没扫到，检查没有真正执行", file=sys.stderr)
        return 1
    bad = []
    for p in files:
        try:
            data = json.loads(p.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError) as e:
            bad.append("%s ⇒ %s" % (p.name, e))
            continue
        if not isinstance(data, dict) or "id" not in data:
            bad.append("%s ⇒ 不是带 id 的对象" % p.name)
    if bad:
        for b in bad:
            print("FAIL  " + b, file=sys.stderr)
        return 1
    print("全部通过（%d 个用例文件都能解析，且都带 id）" % len(files))
    return 0


if __name__ == "__main__":
    sys.exit(main())
