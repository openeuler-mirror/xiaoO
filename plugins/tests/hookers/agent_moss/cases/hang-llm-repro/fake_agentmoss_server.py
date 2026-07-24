#!/usr/bin/env python3
"""假 AgentMoss 服务 — 复现 agent_moss bridge 的 analyze 阶段永久阻塞。

这是旧 audit_agent hang-llm-repro 用例的等价重写。

背景（旧 audit_agent 场景，已废弃）：
  旧 audit.py 用 ThreadPoolExecutor 跑 L3 call_llm，客户报告"http 超时失效"导致
  call_llm 永不返回，worker 永久卡住，进程卡死。旧用例用 sitecustomize 注入把
  call_llm patch 成永久阻塞，验证 audit.py 末尾 os._exit 自愈退出。

agent_moss 新场景：
  agent_moss 判定逻辑迁出为独立常驻 HTTP 服务，xiaoO 只装一个瘦 bridge.py 转发
  到服务。bridge 进程不再跑判定逻辑，没有"卡住的 worker 线程"风险——但它必须保证
  "服务 analyze 阶段永久阻塞时，bridge 自己不卡死"。

  bridge.py 对 /api/v1/analyze 的 urlopen 带 timeout=AGENT_MOSS_HOOK_TIMEOUT，
  超时后抛 URLError → fail-closed deny + exit 0（hook 约定退出码恒 0）。
  本假服务：
    /api/v1/health   → 立即返回 {"status":"healthy"}（让 bridge 活性检查通过）
    /api/v1/analyze  → 永久阻塞（sleep 到被 kill），复现"服务 analyze 阶段挂了"

通过标准（见 repro_hang.sh）：bridge 在 ≈ AGENT_MOSS_HOOK_TIMEOUT 后自愈退出，
  exit 0，stdout 含 {"result": "deny"}，未被外层 timeout 杀掉（即未卡死）。

用法：
    python3 fake_agentmoss_server.py --port 19090 [--analyze-hang-seconds 600]
    # 服务自己不会主动结束 analyze 的阻塞；repro_hang.sh 测完会 kill 掉它。
"""
import argparse
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def make_handler(analyze_hang_seconds: float):
    class _FakeAgentMossHandler(BaseHTTPRequestHandler):
        # 静音默认日志，避免污染 repro 输出
        def log_message(self, fmt, *args):  # noqa: D401
            pass

        def _send_json(self, code: int, obj: dict):
            body = json.dumps(obj).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802
            if self.path.startswith("/api/v1/health"):
                self._send_json(200, {"status": "healthy", "version": "fake-0.0.0"})
                return
            self._send_json(404, {"error": "not found"})

        def do_POST(self):  # noqa: N802
            if self.path.startswith("/api/v1/analyze"):
                # 读空请求体（不关心内容），然后永久阻塞。
                _ = self.rfile.read(int(self.headers.get("Content-Length", 0) or 0))
                # 阻塞 analyze：sleep 到被外部 kill。给个上限只是兜底防僵尸进程。
                time.sleep(analyze_hang_seconds)
                # 真走到这里说明时间到了，仍按 fail-closed 给 deny。
                self._send_json(504, {"decision": "Deny", "reason": "analyze timeout"})
                return
            self._send_json(404, {"error": "not found"})

    return _FakeAgentMossHandler


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=19090)
    ap.add_argument("--host", default="127.0.0.1")
    # analyze 永久阻塞的兜底秒数。默认远大于 repro 的外层 timeout，
    # 保证测试期间 analyze 一直挂着、由 repro_hang.sh kill 收尾。
    ap.add_argument("--analyze-hang-seconds", type=float, default=3600.0)
    args = ap.parse_args()

    srv = ThreadingHTTPServer((args.host, args.port), make_handler(args.analyze_hang_seconds))
    print(f"[fake_agentmoss] listening {args.host}:{args.port} "
          f"(health=ok, analyze hangs {args.analyze_hang_seconds}s)", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        srv.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
