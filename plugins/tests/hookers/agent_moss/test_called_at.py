"""analyze 请求必须带上 called_at，否则 AgentMoss 的 L2 授权时效窗不生效。

AgentMoss 的改密/删用户授权规则要求历史里 ask_user_question 的提问时刻与本次
执行时刻都在时效窗（CONSENT_TTL_SECONDS，默认 300s）内。xiaoO 宿主的 payload
只给毫秒时间戳（`called_at_ms`），bridge 需要转成 ISO 8601 填 `called_at`；
当前动作的 `a_next.called_at` 由 bridge 补当前时刻。

缺了时间 → AgentMoss 退回「最近 5 条」距离兜底，一小时前的授权也算数。
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


def _capture_analyze_body(data):
    """跑一次 _handle_hook_payload，返回 analyze 请求体（health 检查用假响应放行）。"""
    captured = {}

    def fake_urlopen(req, timeout=None):
        url = req.full_url if hasattr(req, "full_url") else str(req)
        if "/api/v1/analyze" in url:
            captured["body"] = json.loads((req.data or b"{}").decode("utf-8"))
            return _FakeResp({"decision": "Allow", "reason": ""})
        return _FakeResp({"status": "healthy", "instance": bridge_module._EXPECT_INSTANCE})

    with patch.object(urllib.request, "urlopen", fake_urlopen), \
         patch.object(bridge_module, "_check_source_install", lambda: None), \
         patch.object(bridge_module, "_resolve_service_url", lambda: "http://127.0.0.1:9090"):
        import contextlib
        import io
        with contextlib.redirect_stdout(io.StringIO()):
            bridge_module._handle_hook_payload(data)
    return captured.get("body")


def _payload(action_history=None, **extra):
    base = {
        "session_id": "s-1",
        "workspace": "/home/user/project",
        "call": {"call_id": "c-2", "tool_name": "bash", "input": {"command": "userdel -r ops"}},
        "action_history": action_history or [],
    }
    base.update(extra)
    return base


# 2026-11-02T10:00:00 UTC = 1762074000000 ms
_ASKED_AT_MS = 1762074000000


def test_history_item_called_at_ms_becomes_iso():
    body = _capture_analyze_body(_payload(action_history=[
        {
            "action_type": "ask_user_question",
            "action_detail": {"questions": [{"question": "确认删除用户 ops 吗？"}]},
            "output": "是",
            "called_at_ms": _ASKED_AT_MS,
        },
    ]))
    assert body is not None, "analyze 请求未被发出"
    called_at = body["action_history"][0].get("called_at")
    assert called_at, "带 called_at_ms 的历史动作必须转出 called_at，否则时效窗失效"
    # 只校验能解析回同一时刻（本地时区 isoformat，不锁定具体格式）
    from datetime import datetime
    assert datetime.fromisoformat(called_at).timestamp() == _ASKED_AT_MS / 1000


def test_history_item_without_timestamp_has_no_called_at():
    body = _capture_analyze_body(_payload(action_history=[
        {"action_type": "ask_user_question", "action_detail": "确认？", "output": "是"},
    ]))
    assert body is not None
    assert "called_at" not in body["action_history"][0], (
        "没有时间就不要伪造 —— 缺失时 AgentMoss 会退回距离窗口"
    )


def test_a_next_carries_current_time():
    body = _capture_analyze_body(_payload())
    assert body is not None
    called_at = body["a_next"].get("called_at")
    assert called_at, "a_next 必须带当前时刻，否则时效窗永远算不出间隔"
    from datetime import datetime
    assert datetime.fromisoformat(called_at)


def main() -> int:
    """无需 pytest：`python3 test_called_at.py` 直接跑（与同目录其它测试一致）。"""
    failures = []
    for fn in (
        test_history_item_called_at_ms_becomes_iso,
        test_history_item_without_timestamp_has_no_called_at,
        test_a_next_carries_current_time,
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
