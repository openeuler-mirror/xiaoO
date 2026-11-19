"""写工具的历史动作必须保留完整参数（含 content）。

AgentMoss 的 RBW-11（本会话脚本生成的文件免先读）依赖历史里脚本写入的
content，判断「脚本是否生成了目标文件」。bridge 过去对 file_write 只留
file_path，content 丢失 → 该豁免在 xiaoO 接入场景静默失效（2026-11 端到端
实测：脚本生成的 CSV 再写被 read_before_write 误拦）。
"""

import json
import sys
from pathlib import Path

_BRIDGE_DIR = Path(__file__).resolve().parents[3] / "hookers" / "agent_moss"
sys.path.insert(0, str(_BRIDGE_DIR))

import bridge as bridge_module


def test_file_write_history_keeps_content():
    item = bridge_module._normalize_history_item({
        "name": "file_write",
        "input": {
            "file_path": "/tmp/am-rbw/gen.py",
            "content": "open('out.csv', 'w').write('x')\n",
        },
    })
    assert item["action_type"] == "file_write"
    detail = json.loads(item["action_detail"])
    assert detail["file_path"] == "/tmp/am-rbw/gen.py"
    assert "out.csv" in detail["content"]


def test_file_edit_history_keeps_content():
    item = bridge_module._normalize_history_item({
        "name": "file_edit",
        "input": {"file_path": "/tmp/a.txt", "old": "a", "new": "b"},
    })
    detail = json.loads(item["action_detail"])
    assert detail["old"] == "a" and detail["new"] == "b"


def test_file_read_history_stays_path_only():
    item = bridge_module._normalize_history_item({
        "name": "file_read",
        "input": {"file_path": "/tmp/a.csv", "content": "not a write"},
    })
    assert item["action_detail"] == "/tmp/a.csv"


def test_bash_history_extracts_command():
    item = bridge_module._normalize_history_item({
        "name": "bash",
        "input": {"command": "cd /tmp && python3 gen.py"},
    })
    assert item["action_detail"] == "cd /tmp && python3 gen.py"


def test_large_content_is_bounded_and_keeps_referenced_names():
    """大文件内容不能整份进历史：截断到上限，但保留文件名供 RBW-11 匹配。"""
    big = "x" * 10000 + "\nwith open('折旧表.csv', 'w') as f:\n    f.write('x')\n"
    item = bridge_module._normalize_history_item({
        "name": "file_write",
        "input": {"file_path": "/tmp/am-rbw/gen.py", "content": big},
    })
    detail = json.loads(item["action_detail"])
    assert len(detail["content"]) <= 2100
    assert detail["content"].endswith("…[truncated]")
    assert "折旧表.csv" in detail["referenced_names"]


def test_input_dict_wins_when_action_detail_is_string():
    """action_detail 是字符串 + input 是字典时，仍要拿到 content（否则 RBW-11 失效）。"""
    item = bridge_module._normalize_history_item({
        "name": "file_write",
        "action_detail": "/tmp/am-rbw/gen.py",
        "input": {"file_path": "/tmp/am-rbw/gen.py", "content": "open('out.csv', 'w')"},
    })
    detail = json.loads(item["action_detail"])
    assert detail["file_path"] == "/tmp/am-rbw/gen.py"
    assert "out.csv" in detail["content"]
    assert "out.csv" in detail["referenced_names"]
