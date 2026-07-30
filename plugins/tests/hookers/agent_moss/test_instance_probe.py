#!/usr/bin/env python3
"""bridge 探测归属过滤测试

验证 _probe_service_url 只认 instance 匹配的 agentmoss，避免漂移到
同机其他 agent（如 OpenDesk）spawn 的 agentmoss 实例。

测试方式：
    cd plugins/tests/hookers/agent_moss
    python3 test_instance_probe.py
"""

import json
import sys
import urllib.request
from pathlib import Path
from unittest.mock import patch

# bridge.py 在 plugins/hookers/agent_moss/bridge.py，测试在 plugins/tests/hookers/agent_moss/。
# 共享前缀 plugins（parents[3]），bridge 目录 = plugins/hookers/agent_moss。
_BRIDGE_DIR = Path(__file__).resolve().parents[3] / "hookers" / "agent_moss"
sys.path.insert(0, str(_BRIDGE_DIR))

import bridge as bridge_module


class _FakeResp:
    """模拟 urlopen 返回的响应对象。"""

    def __init__(self, body_dict):
        self._body = json.dumps(body_dict).encode()
        self.status = 200

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def read(self):
        return self._body


def _make_urlopen(responses):
    """构造按 port 顺序返回不同 health 响应的假 urlopen。

    responses: dict {port: body_dict_or_None}，None 表示该端口无监听（抛异常）。
    """

    call_state = {"port_idx": 0}

    def fake_urlopen(req, timeout=None):
        # 从 url 里提取 port
        url = req.full_url if hasattr(req, "full_url") else str(req)
        port = None
        for p in [9090, 9091, 9092, 9093, 9094, 9095]:
            if f":{p}/" in url or url.endswith(f":{p}"):
                port = p
                break
        body = responses.get(port)
        if body is None:
            raise ConnectionError(f"no listener on {port}")
        return _FakeResp(body)

    return fake_urlopen


def test_probe_matches_instance():
    """instance==xiaoo 的端口应被选中。"""
    responses = {
        9090: {"status": "healthy", "version": "0.10.2", "instance": "xiaoo"},
    }
    fake = _make_urlopen(responses)
    with patch.object(bridge_module.urllib.request, "urlopen", fake):
        url = bridge_module._probe_service_url()
    assert url == "http://127.0.0.1:9090", f"应命中 9090，实际 {url}"
    print("  ✓ instance 匹配的端口被选中")


def test_probe_skips_non_matching_instance():
    """OpenDesk 的 agentmoss（instance=opendesk）应被跳过，不漂移。"""
    responses = {
        9090: {"status": "healthy", "version": "0.10.2", "instance": "opendesk"},
        9091: {"status": "healthy", "version": "0.10.2", "instance": "xiaoo"},
    }
    fake = _make_urlopen(responses)
    with patch.object(bridge_module.urllib.request, "urlopen", fake):
        url = bridge_module._probe_service_url()
    assert url == "http://127.0.0.1:9091", f"应跳过 9090(opendesk) 命中 9091(xiaoo)，实际 {url}"
    print("  ✓ instance 不匹配的端口被跳过")


def test_probe_skips_empty_instance():
    """未带 instance 字段（旧版/未配置）的端口应被跳过（严格过滤）。"""
    responses = {
        9090: {"status": "healthy", "version": "0.10.0"},  # 无 instance 字段
        9091: {"status": "healthy", "version": "0.10.2", "instance": "xiaoo"},
    }
    fake = _make_urlopen(responses)
    with patch.object(bridge_module.urllib.request, "urlopen", fake):
        url = bridge_module._probe_service_url()
    assert url == "http://127.0.0.1:9091", f"应跳过无 instance 的 9090 命中 9091，实际 {url}"
    print("  ✓ 未带 instance（旧版）的端口被跳过")


def test_probe_none_when_no_match():
    """全是别人的 instance → 探测失败返回 None（走 fail-closed）。"""
    responses = {
        9090: {"status": "healthy", "version": "0.10.2", "instance": "opendesk"},
        9091: {"status": "healthy", "version": "0.10.2", "instance": "other-agent"},
    }
    fake = _make_urlopen(responses)
    with patch.object(bridge_module.urllib.request, "urlopen", fake):
        url = bridge_module._probe_service_url()
    assert url is None, f"无匹配应返回 None，实际 {url}"
    print("  ✓ 无匹配实例时返回 None（fail-closed）")


def test_probe_skips_unhealthy():
    """status 非 healthy 的端口应被跳过（原有行为不变）。"""
    responses = {
        9090: {"status": "degraded", "version": "0.10.2", "instance": "xiaoo"},
        9091: {"status": "healthy", "version": "0.10.2", "instance": "xiaoo"},
    }
    fake = _make_urlopen(responses)
    with patch.object(bridge_module.urllib.request, "urlopen", fake):
        url = bridge_module._probe_service_url()
    assert url == "http://127.0.0.1:9091", f"应跳过 degraded 的 9090，实际 {url}"
    print("  ✓ 非 healthy 的端口被跳过")


def main():
    print("=== bridge 探测归属过滤测试 ===")
    # 确保用 xiaoo 期望值（env 可能未设）
    bridge_module._EXPECT_INSTANCE = "xiaoo"
    test_probe_matches_instance()
    test_probe_skips_non_matching_instance()
    test_probe_skips_empty_instance()
    test_probe_none_when_no_match()
    test_probe_skips_unhealthy()
    print("\n✅ 全部通过")


if __name__ == "__main__":
    main()
