#!/usr/bin/env python3
"""假 LLM server：接受 TCP 连接但永远不发 HTTP response。

模拟客户卡死场景的根因——OpenAI SDK 的 httpx read 超时在此失效：
TCP 连接建立了，但 server 永远不回 response header/body，
SDK 卡在等待响应数据上（某些 httpx 配置下 read timeout 不触发，
或被连接 keep-alive/重试逻辑绕过），造成永久阻塞。

用法：python3 fake_llm_server.py <port>
监听 port，每个连接 accept 后 sleep 永不返回（连 response header 都不发）。
"""
import socket
import sys
import threading
import time


def handle_conn(conn, addr):
    # 慢速发送攻击：接受连接后，每隔几秒发一个字节（但永远不发完整 HTTP response）。
    # 这样 OpenAI SDK 的 httpx read timeout 会被不断重置（它看到"有数据在来"），
    # 永远凑不齐一个完整 response 的判断，从而真正复现"HTTP 超时失效"——
    # worker 线程里的 call_llm 永久卡在等 response 上，不会因 read timeout 退出。
    # 这才是 shutdown(wait=True) 盲区能被触发的条件。
    try:
        conn.settimeout(2.0)
        try:
            while True:
                data = conn.recv(4096)
                if not data:
                    break
        except socket.timeout:
            pass  # 读完请求体了
        # 慢速滴答：每 3 秒发一个空格，让 httpx 以为 response 在慢速传输中
        # （read timeout 因持续有数据而不断重置，永不触发）。
        while True:
            try:
                conn.sendall(b" ")
            except Exception:
                break
            time.sleep(3)
    except Exception:
        pass
    finally:
        try:
            conn.close()
        except Exception:
            pass


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18799
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(8)
    print(f"[fake_llm] listening on 127.0.0.1:{port} (accept but never respond)", flush=True)
    while True:
        conn, addr = srv.accept()
        print(f"[fake_llm] accepted from {addr}, will hang forever", flush=True)
        t = threading.Thread(target=handle_conn, args=(conn, addr), daemon=True)
        t.start()


if __name__ == "__main__":
    main()
