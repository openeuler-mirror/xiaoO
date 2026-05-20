# xiaoO AgentOS记忆系统 x LMCache：语义感知 KV Cache 协同说明文档

## 功能介绍

xiaoO 已经内嵌了 KVCache 感知调度能力，通过配置文件中的 `kvcache_enabled` 标志位开启，在请求前会根据请求中的 `chunk_hashes` 来判断是否需要从 KVCache 中读取缓存，在上下文变化时会根据 `diff_deleted` 方法来判断是否需要删除 KVCache 中的缓存

## 示例配置
```
[llm]
provider = "local"
model = "glm4.7"
api_base = "http://localhost:8080/v1"
max_tokens = 20000
context_window = 128000
reasoning_effort = "off"
kvcache_enabled = true
kvcache_debug_enabled = false   # 打开 debug 文件生成
```

## LMCache-Ascend 部分说明

### 环境配置

| 组件 | 版本 / 分支 | 说明 |
|------|------------|------|
| 容器镜像 | `quay.io/ascend/vllm-ascend:v0.18.0rc1-a3` | 基于 Ascend NPU 的 vLLM 推理容器 |
| LMCache | `v0.4.3` | 上游 LMCache 核心，不做任何修改 |
| LMCache-Ascend | `main` | 本次新增功能所在仓库 |

### 启动推理服务
```bash
# 启动容器
export IMAGE=quay.io/ascend/vllm-ascend:v0.18.0rc1-a3
docker run --privileged \
    --name agentos-lmcache-ascend \
    --net=host \
    --ipc=host \
    --device /dev/davinci0 \
    --device /dev/davinci1 \
    --device /dev/davinci2 \
    --device /dev/davinci3 \
    --device /dev/davinci4 \
    --device /dev/davinci5 \
    --device /dev/davinci6 \
    --device /dev/davinci7 \
    --device /dev/davinci_manager \
    --device /dev/devmm_svm \
    --device /dev/hisi_hdc \
    -v /dev/shm:/dev/shm \
    -v /etc/hccn.conf:/etc/hccn.conf \
    -v /usr/bin/hccn_tool:/usr/bin/hccn_tool \
    -v /usr/local/dcmi:/usr/local/dcmi \
    -v /usr/local/Ascend/driver/tools/hccn_tool:/usr/local/Ascend/driver/tools/hccn_tool \
    -v /usr/local/bin/npu-smi:/usr/local/bin/npu-smi \
    -v /usr/local/Ascend/driver/lib64/:/usr/local/Ascend/driver/lib64/ \
    -v /usr/local/Ascend/driver/version.info:/usr/local/Ascend/driver/version.info \
    -v /etc/ascend_install.info:/etc/ascend_install.info \
    -v /root/.cache:/root/.cache \
    -v /root/.pip:/root/.pip \
    -v /workspace:/workspace \
    -v /mnt/sdb/models:/mnt/sdb/models \
    -it $IMAGE bash

# 安装 LMCache
git clone https://github.com/LMCache/LMCache.git
cd LMCache && git checkout v0.4.3 && export NO_CUDA_EXT=1
python3 -m pip install -v --no-build-isolation -e .

# 安装 LMCache-Ascend（语义感知 KV Cache 调度特性已合入master）
git clone https://github.com/LMCache/LMCache-Ascend.git
cd LMCache-Ascend && unset SOC_VERSION
git submodule update --init --recursive
python3 -m pip install -v --no-build-isolation -e .

# 启动glm4.7服务
#!/bin/bash
export PYTHONHASHSEED=0
export HCCL_BUFFSIZE=512
export OMP_PROC_BIND=false
export OMP_NUM_THREADS=1
export PYTORCH_NPU_ALLOC_CONF=expandable_segments:True
export HCCL_OP_EXPANSION_MODE=AIV
export VLLM_ASCEND_BALANCE_SCHEDULING=1
export VLLM_ASCEND_ENABLE_TOPK_OPTIMIZE=1
export VLLM_ASCEND_ENABLE_FLASHCOMM1=1
export VLLM_ASCEND_ENABLE_FUSED_MC2=1

export LMCACHE_CONFIG_FILE="/home/lmcache_config.yaml"

timestamp=$(date "+%Y%m%d%H%M%S")

vllm serve /mnt/sdb/models/GLM-4.7 \
--host 0.0.0.0 \
--port 8080 \
--data-parallel-size 1 \
--tensor-parallel-size 16 \
--seed 1024 \
--served-model-name glm4.7 \
--enable-expert-parallel \
--max-num-seqs 16 \
--max-model-len 133000 \
--max-num-batched-tokens 8192 \
--trust-remote-code \
--gpu-memory-utilization 0.92 \
--async-scheduling \
--tool-call-parser glm47 \
--reasoning-parser glm45 \
--enable-auto-tool-choice \
--load-format=dummy \
--no-enable-prefix-caching \
--compilation-config '{"cudagraph_mode": "FULL_DECODE_ONLY"}' \
--kv-transfer-config '{"kv_connector":"LMCacheAscendConnectorV1Dynamic","kv_role":"kv_both", "kv_connector_module_path":"lmcache_ascend.integration.vllm.lmcache_ascend_connector_v1"}' \
2>&1 | tee logs/vllm_base_glm47_$timestamp.log

# 驱逐接口调用示例，从ddr驱逐三个 chunk，如果要从ssd驱逐，需要添加 location，或者不指定 location 将chunk完全删除
curl -s -X POST http://localhost:6999/memory/evict \
  -H "Content-Type: application/json" \
  -d "{\"chunk_hashes\": [\"-6ed9a448787a198f\",\"-4ca98d8c9e52152e\",\"3f5c45ecbab5e9fd\"], \"locations\": [\"LocalCPUBackend\"]}"

# 预取接口调用示例，从ssd预取三个 chunk到ddr
curl -s -X POST http://localhost:6999/memory/prefetch \
   -H "Content-Type: application/json" \
   -d "{\"chunk_hashes\": [\"-6ed9a448787a198f\",\"-4ca98d8c9e52152e\",\"3f5c45ecbab5e9fd\"], \"lookup_id\": \"my_prefetch_001\"}"
```

---

### 一、问题背景

大模型推理中，KV Cache 是决定延迟的核心瓶颈。当前 LMCache-Ascend 已经实现了多层存储（GPU HBM → CPU DDR → Local SSD）的自动搬运，但存在一个根本架构短板：

> **LMCache 知道"缓存存了哪些 KV chunk"，但上层记忆系统才知道"哪些 chunk 即将被复用"。**

缺失的能力：
- 请求结束后，上层记忆系统无法获知**这一批 prompt 对应哪些 KV chunk**
- 下一个请求到来前，上层记忆系统无法**主动预取**即将复用的 chunk
- 无法**主动驱逐**不再需要的 chunk 以释放 DDR 空间

本方案以 `chunk_hash` 为全局标识符，在 LMCache-Ascend 与上层记忆系统之间建立两条管控通道：
1. **回写通道**：请求结束时通过 `kv_transfer_params` 返回 `chunk_hashes`
2. **控制通道**：通过 REST API `/memory/prefetch` 和 `/memory/evict` 接收上层调度指令

---

### 二、总体架构

```
上层记忆系统
  │
  │  "继续讨论刚才的话题" → intent → 查映射表 → [hash_a, hash_b]
  │
  ├── POST /memory/evict(hash_old)   ──→  驱逐无用 chunk
  ├── POST /memory/prefetch(hash_a, hash_b) ──→  预取复用 chunk
  │
  ▼
vLLM 推理引擎
  │  推理结束后返回 kv_transfer_params:
  │  {"chunk_hashes": ["hash_x", "hash_y", ...]}
  │
  ▼
LMCache-Ascend 存储引擎 (GPU HBM ←→ CPU DDR ←→ SSD)
```

chunk_hash 是贯穿全链路的全局标识符：
- **确定性**：相同 token 序列 + 相同 `PYTHONHASHSEED` → 相同 hash
- **前缀依赖**：hash[i] = blake3(hash[i-1] || tokens[cs:ce])，链式结构防篡改
- **纯 CPU**：blake3 微秒级完成，请求结束重算亚毫秒开销

---

### 三、chunk_hash 的返回机制

#### 3.1 数据流

```
Prefill 阶段:
  lookup_client.lookup(token_ids=prompt_tokens)
    ├── token_database.process_tokens() → 计算 [hash_0, hash_1, ...]
    ├── _lookup_hashes_cache[req_id] = [hex(h) for h in hashes]   ← 缓存到内存
    └── ZMQ → Worker → 后端查询/预取

... 推理执行 ...

请求结束:
  request_finished()
    ├── lookup_client.get_cached_hashes(req_id)  ← 直接 pop 缓存，零计算
    └── 返回 {"chunk_hashes": ["abc01", "def02", ...]} → kv_transfer_params
```

#### 3.2 实现细节

**改动 1：`LMCacheAscendAsyncLookupClient`（新建文件）**

继承 `LMCacheAsyncLookupClient`，在 `lookup()` 方法中缓存 chunk hashes：

```python
class LMCacheAscendAsyncLookupClient(LMCacheAsyncLookupClient):
    def lookup(self, token_ids, lookup_id, ...):
        # ... 原有逻辑 ...
        # 新增一行：缓存 hex hashes
        self._lookup_hashes_cache[lookup_id] = [f"{h:x}" for h in hashes]
        # ... 其余不变 ...

    def get_cached_hashes(self, lookup_id):
        # pop = 取走即清理，不泄漏内存
        return self._lookup_hashes_cache.pop(lookup_id, None)
```

**关键设计决策**：
- `get_cached_hashes` 使用 `.pop()` — 取走即清理
- `clear_lookup_status` 不清 hashes 缓存 — 因为它在 `update_state_after_alloc()` 中被调用（早于 `request_finished`），提前清理会导致缓存丢失
- `__init__` 禁止直接实例化 — 通过 `from_existing()` 原地升级已创建的实例，避免 ZMQ socket/线程状态丢失

**改动 2：`vllm_v1_adapter.py` — `_upgrade_lookup_client()`**

在 connector 初始化时将已创建的 `LMCacheAsyncLookupClient` 原地升级：

```
LMCacheManager.__init__()
  └─ factory.create_lookup_client()
       → LMCacheAsyncLookupClient()         ← 普通类
       → HitLimitLookupClient(client)        ← 装饰器
       → ChunkStatisticsLookupClient(client) ← 装饰器

LMCacheAscendConnectorV1Impl.__init__()
  └─ super().__init__()  # 上述 lookup_client 创建完成
  └─ _upgrade_lookup_client()
       └─ 穿透装饰链 → 找到 LMCacheAsyncLookupClient
          └─ from_existing() → __class__ = LMCacheAscendAsyncLookupClient
```

**改动 3：`vllm_v1_adapter.py` — `request_finished()` 返回 chunk_hashes**

```python
def request_finished(self, request, block_ids):
    _, return_params = super().request_finished(request, block_ids)

    if hasattr(request, "all_token_ids") and request.all_token_ids:
        # 优先：从 lookup_client 缓存取（零成本）
        inner = self.lookup_client
        while hasattr(inner, "actual_lookup_client"):
            inner = inner.actual_lookup_client
        if hasattr(inner, "get_cached_hashes"):
            new_hashes = inner.get_cached_hashes(request.request_id)

        # 兜底：缓存未命中时才全量重算
        if new_hashes is None:
            new_hashes = [
                f"{hash_val:x}"
                for _, _, hash_val in
                self._token_db.process_tokens(
                    tokens=request.prompt_token_ids or request.all_token_ids
                )
            ]

        if new_hashes:
            return_params = return_params or {}
            return_params["chunk_hashes"] = new_hashes

    return delay_free, return_params
```

**两阶段覆概率分析**：
- 首页 | 命中 `get_cached_hashes()` | ~100%
- 兜底 | `ChunkedTokenDatabase.process_tokens()` | 极低（首次预填逻辑异常时触发）

---

### 四、REST API：prefetch 与 evict

#### 4.1 接口定义

##### `POST /memory/prefetch`

上层记忆系统将已命中的 chunk_hashes 传给 LMCache-Ascend，LMCache-Ascend 提前从 SSD 加载到 DDR。

```json
// Request
POST /memory/prefetch
{
    "chunk_hashes": ["a1b2c3d4...", "e5f6g7h8..."],
    "lookup_id": "req_001"
}

// Response
{
    "status": "prefetch_started",
    "lookup_id": "req_001",
    "num_chunks": 2
}
```

内部调用链：
```
prefetch()
  → _hashes_to_keys()                  # hex hash → CacheEngineKey
  → storage_manager.async_lookup_and_prefetch()
      → batched_async_contains()       # 检查 key 是否存在
      → batched_get_non_blocking()     # 异步 SSD→DDR 传输
      → prefetch_all_done_callback()
          → hot_cache 注册             # 后续检索直读 DDR
          → selective unpin            # 按 backend 释放 pin
          → send_response_to_scheduler()
```

##### `POST /memory/evict`

上层记忆系统驱逐不需要的 KV cache。

```json
// Request
POST /memory/evict
{
    "chunk_hashes": ["a1b2c3d4..."],
    "locations": ["LocalCPUBackend"]   // 可选，默认所有 backend
}

// Response
{
    "status": "success",
    "num_evicted": 1
}
```

#### 4.2 多进程转发

在 vLLM v1 多进程模式下，Scheduler 进程不持有 `LMCacheEngine`，但持有 API Server。转发判断逻辑：

```
_get_engine(request):
  if lmcache_engine exists:      → 直接处理（Worker 进程）
  if port_offset == 0:           → 转发到所有 Worker（Scheduler 进程）
  else:                          → 返回 503
```

转发实现在 `_forward_to_all_workers()` 中完成——对所有 Worker 端口并行发送 `urllib` 请求，合并结果：

```
AgentOS → POST scheduler:6999/memory/prefetch
            ├── Worker 0:7000/memory/prefetch
            └── Worker 1:7001/memory/prefetch
            ← 合并响应
```

#### 4.3 端口编排

```
port_start = 6999 (默认)

Scheduler  (port_offset=0)   → :6999
Worker 0   (port_offset=1)   → :7000
Worker 1   (port_offset=2)   → :7001
...
```

---

### 五、storage_manager 增强

#### 5.1 hot_cache 注册

prefetch 完成后，将 MemoryObj 注册到 `LocalCPUBackend.hot_cache`：后续同步检索和 vLLM lookup 通过前缀匹配直接命中，绕过磁盘 I/O。

#### 5.2 selective unpin

**问题**：`batched_async_contains(pin=True)` 只在特定 backend 上 pin 了 key，如果全局 `batched_unpin(all_keys)` 会对所有 backend 尝试 unpin，引发 double-unpin 错误。

**解决**：在 `async_lookup_and_prefetch` 中追踪 `loading_task_backends`，prefetch 完成后只在这些 backend 上调 `unpin()`。

#### 5.3 DiskBackend pin 释放

`LocalDiskBackend.batched_get_non_blocking()` 会在新分配的 DDR buffer 上调 `memory_obj.pin()`。对象注册到 hot_cache 后，后续检索不再经过 DiskBackend，此 pin 可以释放。

---

### 六、交互时序全景

```
上层记忆系统                  vLLM                        LMCache
     │                          │                            │
     │ ① 语义分析               │                            │
     │   prompt → intent        │                            │
     │   查映射表 → [h1, h2]    │                            │
     │                          │                            │
     │ ② 驱逐旧话题 chunk        │                            │
     │ ─── POST /memory/evict ────────────────────────────▶  │
     │                          │          batched_remove()  │
     │                          │                            │
     │ ③ 预取复用 chunk          │                            │
     │ ─── POST /memory/prefetch ──────────────────────────▶ │
     │                          │  async_lookup_and_prefetch │
     │                          │   SSD→DDR (异步, 非阻塞)   │
     │                          │   hot_cache 注册           │
     │                          │                            │
     │ ④ 发起推理请求            │                            │
     │ ─── POST /chat ────────▶ │                            │
     │                          │  lookup + retrieve ──────▶ │
     │                          │                   cache hit │
     │ ◄── SSE stream ──────── │                            │
     │                          │                            │
     │ ◄── [DONE] +             │                            │
     │    {"chunk_hashes":       │                            │
     │     ["hx","hy",...]}     │                            │
     │                          │                            │
     │ ⑤ 回写映射表              │                            │
     │   intent → [hx, hy, ...]  │                            │
```

> 步骤 ②③ 在推理请求之前执行，prefetch 是异步非阻塞的，不影响任何推理主流程。

---

### 七、修改清单

#### LMCache 核心：零改动

LMCache `v0.4.3` 代码不做任何修改。所有新增能力通过 lmcache-ascend 的 monkey-patch 机制注入。

#### lmcache-ascend 新增文件

| 文件 | 行数 | 功能 |
|------|------|------|
| `v1/lookup_client/lmcache_ascend_async_lookup_client.py` | ~70 | 子类：lookup 时缓存 chunk hashes，提供 `get_cached_hashes()` |
| `v1/lookup_client/__init__.py` | 1 | 包初始化 |
| `v1/internal_api_server/__init__.py` | 1 | 包初始化 |
| `v1/internal_api_server/memory/__init__.py` | 1 | 包初始化 |
| `v1/internal_api_server/memory/memory_api.py` | ~299 | `/memory/prefetch` + `/memory/evict` REST API |

#### lmcache-ascend 修改文件

| 文件 | 改动 | 功能 |
|------|------|------|
| `__init__.py` | +2 个 `_patch_*` 函数 + 调用 | `_patch_storage_manager_agentos`：hot_cache + selective unpin；`_patch_api_server_agentos`：注册 memory router + app.state 端口变量 |
| `integration/vllm/vllm_v1_adapter.py` | `_upgrade_lookup_client()` + `request_finished()` 增强 | 注入 Ascend 子类 + 返回 chunk_hashes |

---

### 八、测试验证

#### 代码质量

`pre-commit run --all-files` 所有 hook 通过：
- check-spdx-header ✅
- ruff (E/F/B) ✅
- isort ✅
- ruff-format ✅
- codespell ✅
- clang-format ✅

#### 单元测试

| 测试文件 | 覆盖内容 |
|---------|---------|
| `tests/v1/lookup_client/test_lmcache_ascend_async_lookup_client.py` | `from_existing` 升级、`get_cached_hashes` 取/弹/空/选择性清理、`lookup` 缓存 hex hash/覆盖/空 tokens、`clear_lookup_status` 不清 hashes |
| `tests/v1/internal_api_server/test_memory_api.py` | `_hashes_to_keys` 键构造、prefetch 缺字段/无 storage/成功路径、evict 缺字段/无 storage/成功路径 |