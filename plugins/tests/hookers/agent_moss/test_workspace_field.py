"""analyze 请求里的 cwd 必须来自宿主 payload 的 workspace 字段。

xiaoO 的 tool-pre payload 用 `workspace` 承载工作区路径（没有 `cwd` 字段）。
bridge 过去只读 `data.get("cwd", "")`，取值恒为空串，AgentMoss 的层2
「间接文件访问检测」因此在 xiaoO 接入场景下静默失效。
"""

import json
import sys
import urllib.request
from pathlib import Path
from unittest.mock import patch

_BRIDGE_DIR = Path(__file__).resolve().parents[3] / "hookers" / "agent_moss"
sys.path.insert(0, str(_BRIDGE_DIR))

import bridge as bridge_module


class _FakeResp:
    def __init__(self, body_dict):
        self._body = json.dumps(body_dict).encode()
        self.status = 200

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def read(self):
        return self._body


def _capture_analyze_body(data, monkeypatch_target="urllib.request.urlopen"):
    """跑一次 _handle_hook_payload，返回 analyze 请求体（health 检查用假响应放行）。"""
    captured = {}

    def fake_urlopen(req, timeout=None):
        url = req.full_url if hasattr(req, "full_url") else str(req)
        if "/api/v1/analyze" in url:
            captured["body"] = json.loads((req.data or b"{}").decode("utf-8"))
            return _FakeResp({"decision": "Allow", "reason": ""})
        # health / 探测类请求
        return _FakeResp({"status": "healthy", "instance": bridge_module._EXPECT_INSTANCE})

    with patch(monkeypatch_target, fake_urlopen), \
         patch.object(bridge_module, "_check_source_install", lambda: None), \
         patch.object(bridge_module, "_resolve_service_url", lambda: "http://127.0.0.1:9090"):
        import io, contextlib
        with contextlib.redirect_stdout(io.StringIO()):
            bridge_module._handle_hook_payload(data)
    return captured.get("body")


def _payload(**extra):
    base = {
        "session_id": "s-1",
        "workspace": "/home/user/project",
        "call": {"call_id": "c-1", "tool_name": "read_file", "input": {"file_path": "/etc/passwd"}},
    }
    base.update(extra)
    return base


def test_cwd_comes_from_workspace_when_host_omits_cwd():
    body = _capture_analyze_body(_payload())
    assert body is not None, "analyze 请求未被发出"
    assert body["cwd"] == "/home/user/project", (
        "宿主只发 workspace；cwd 为空说明层2 的间接文件访问检测拿不到工作区"
    )


def test_explicit_cwd_still_wins():
    body = _capture_analyze_body(_payload(cwd="/other/place"))
    assert body is not None
    assert body["cwd"] == "/other/place", "仍自带 cwd 的调用方不应被改变"


def test_missing_workspace_stays_empty_not_absent():
    p = _payload()
    p.pop("workspace")
    body = _capture_analyze_body(p)
    assert body is not None
    assert body["cwd"] == "", "没有工作区时保持空串（AgentMoss 侧的既有语义）"


def main() -> int:
    """无需 pytest：`python3 test_workspace_field.py` 直接跑（与同目录其它测试一致）。"""
    failures = []
    for fn in (
        test_cwd_comes_from_workspace_when_host_omits_cwd,
        test_explicit_cwd_still_wins,
        test_missing_workspace_stays_empty_not_absent,
    ):
        try:
            fn()
            print(f"  ok   {fn.__name__}")
        except AssertionError as exc:
            failures.append((fn.__name__, exc))
            print(f"  FAIL {fn.__name__}: {exc}")
    if failures:
        print(f"\n{len(failures)} 个用例失败")
        return 1
    print("\n全部通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
