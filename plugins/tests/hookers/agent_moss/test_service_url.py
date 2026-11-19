"""服务地址解析：port 文件优先、多实例取最高版本。

2026-11 实测：同机残留旧 AgentMoss（0.17.22 占 9090，token 不同）时，
bridge 按端口顺序探测会选中旧实例，所有工具调用 401 被 fail-closed 拒绝。
新版本漂移到 9091，并写 agent_moss_server.port.json；bridge 必须优先读 port 文件，
探测时也要在多实例中取版本最高的。
"""

import json
import sys
from pathlib import Path

_BRIDGE_DIR = Path(__file__).resolve().parents[3] / "hookers" / "agent_moss"
sys.path.insert(0, str(_BRIDGE_DIR))

import bridge as bridge_module


def _reset(monkeypatch):
    monkeypatch.setattr(bridge_module, "_cached_service_url", None, raising=False)
    monkeypatch.setattr(bridge_module, "_URL_ENV", "", raising=False)
    monkeypatch.delenv("AGENT_MOSS_URL", raising=False)
    monkeypatch.delenv("AGENT_MOSS_PORT", raising=False)


def test_port_file_wins_over_stale_9090(tmp_path, monkeypatch):
    _reset(monkeypatch)
    monkeypatch.setenv("AGENT_MOSS_DATA_DIR", str(tmp_path))
    (tmp_path / "xiaoo").mkdir()
    (tmp_path / "xiaoo" / "agent_moss_server.port.json").write_text(
        json.dumps({"port": 9091, "pid": 1, "instance": "xiaoo"}), encoding="utf-8"
    )
    seen = {}

    def fake_healthy(port):
        seen["port"] = port
        if port == 9091:
            return {"status": "healthy", "instance": "xiaoo", "version": "0.17.33"}
        return None

    monkeypatch.setattr(bridge_module, "_healthy_instance", fake_healthy)
    assert bridge_module._resolve_service_url() == "http://127.0.0.1:9091"
    assert seen["port"] == 9091


def test_probe_prefers_highest_version(tmp_path, monkeypatch):
    _reset(monkeypatch)
    monkeypatch.setenv("AGENT_MOSS_DATA_DIR", str(tmp_path))  # 无 port 文件

    def fake_healthy(port):
        if port == 9090:
            return {"status": "healthy", "instance": "xiaoo", "version": "0.17.22"}
        if port == 9091:
            return {"status": "healthy", "instance": "xiaoo", "version": "0.17.33"}
        return None

    monkeypatch.setattr(bridge_module, "_healthy_instance", fake_healthy)
    assert bridge_module._resolve_service_url() == "http://127.0.0.1:9091"


def test_version_key_order():
    assert bridge_module._version_key("0.17.33") > bridge_module._version_key("0.17.22")
    assert bridge_module._version_key("0.9.9") < bridge_module._version_key("0.17.0")
