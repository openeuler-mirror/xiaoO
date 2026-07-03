"""Monkeypatch 注入：把 L3 用的 call_llm 替换成永久阻塞，确定性复现客户卡死。

客户卡死的等效条件："call_llm 永不返回"（http 超时失效的最终效果）。
L3 的 ThreadPoolExecutor worker 跑 call_llm，若它永不返回：
  - master 老代码（with ThreadPoolExecutor）：future.result 超时抛异常后，
    __exit__ 的 shutdown(wait=True) 等那个永久卡住的 worker → 永久卡死。
  - 修复后代码（shutdown(wait=False)）：不等，立即返回 → L3 自愈。

时序要点：audit.py 的 `from audit_policy_checker.main import audit_action`
会触发 llm_analyzer 被 import，此时它 `from ..llm_client import call_llm`
把 call_llm 绑定到自己模块命名空间。必须在 llm_analyzer 完成 import 后、
analyze() 被调前，patch llm_analyzer.call_llm（而非 llm_client.call_llm，
因为 llm_analyzer 已绑定本地引用）。

用 sitecustomize（PYTHONPATH 最前）装一个 import hook，捕获 llm_analyzer 的
import 时机，在其 loaded 后立即 patch。
"""
import sys
import threading
import importlib.abc
import importlib.machinery


_HANG_EVENT = threading.Event()


def _hung_call_llm(*args, **kwargs):
    # 永久阻塞，模拟 call_llm 永不返回
    _HANG_EVENT.wait()
    raise RuntimeError("unreachable: hung call_llm returned")


class _LlmAnalyzerPatcher(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    """import hook：拦截 audit_policy_checker.security.llm_analyzer，
    让它正常 import 后立即 patch 其 call_llm。"""

    _TARGET = "audit_policy_checker.security.llm_analyzer"

    def find_spec(self, fullname, path, target=None):
        if fullname != self._TARGET:
            return None
        # 用默认机制加载真实模块
        sys.meta_path.remove(self)
        try:
            spec = importlib.machinery.PathFinder.find_spec(fullname, path)
        finally:
            sys.meta_path.insert(0, self)
        if spec is None:
            return None
        # 包一层 loader，加载完后 patch
        orig_loader = spec.loader
        spec.loader = _PatchingLoader(orig_loader)
        return spec


class _PatchingLoader(importlib.abc.Loader):
    def __init__(self, orig):
        self._orig = orig

    def create_module(self, spec):
        if hasattr(self._orig, "create_module"):
            return self._orig.create_module(spec)
        return None

    def exec_module(self, module):
        self._orig.exec_module(module)
        # llm_analyzer 加载完成，立即 patch 它绑定的 call_llm
        if hasattr(module, "call_llm"):
            module.call_llm = _hung_call_llm
            sys.stderr.write("[inject_hang] patched llm_analyzer.call_llm to hang forever\n")
        else:
            sys.stderr.write("[inject_hang] WARN: llm_analyzer has no call_llm attr\n")


def _install():
    sys.meta_path.insert(0, _LlmAnalyzerPatcher())
    sys.stderr.write("[inject_hang] import hook installed\n")


_install()
