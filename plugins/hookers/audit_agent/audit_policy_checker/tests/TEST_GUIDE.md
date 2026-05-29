# AuditAgent 测试指南

## 目录

1. [环境准备](#1-环境准备)
2. [运行测试](#2-运行测试)
3. [LLM 配置](#3-llm-配置)
4. [测试用例说明](#4-测试用例说明)
5. [常见问题](#5-常见问题)

---

## 1. 环境准备

### 1.1 前置依赖

| 依赖 | 最低版本 | 检查命令 |
|------|---------|---------|
| Rust | 1.70+ | `rustc --version` |
| Python | 3.10+ | `python3 --version` |
| Linux | 5.13+ | `uname -r`（cerberus 测试需要） |

### 1.2 编译 xiaoo

```bash
cd /path/to/xiaoO
cargo build --release
```

验证二进制存在：

```bash
ls target/release/xiaoo && echo "OK"
```

### 1.3 验证审计插件

```bash
# 确认 plugin.json 存在
ls plugins/hookers/audit_agent/plugin.json

# 确认 Python 虚拟环境正常
plugins/hookers/audit_agent/audit_policy_checker/venv/bin/python3 -c "print('ok')"
```

### 1.4 确认 LLM 可用

测试脚本会自动复用你已有的 xiaoo LLM 配置（`~/.config/xiaoo/config.toml`）。

先查看你的配置中 `api_key_env` 是什么：

```bash
grep api_key_env ~/.config/xiaoo/config.toml
```

然后设置对应的环境变量并验证：

```bash
# 将 YOUR_API_KEY 替换为你的实际 key
# 将 ZHIPU_API_KEY 替换为你 config.toml 中 api_key_env 的值
export ZHIPU_API_KEY="YOUR_API_KEY"

# 验证 LLM 可用
./target/release/xiaoo run -p "你好，回复两个字"
```

如果能正常回复，说明 LLM 已配好，直接跳到 [第2节](#2-运行测试)。
如果还没有配置 LLM，参考 [第3节](#3-llm-配置)。

---

## 2. 运行测试

### 2.1 一条命令跑全部 rules 测试

```bash
cd plugins/hookers/audit_agent/audit_policy_checker/tests/xiaoo

# 只需提供 api_key，LLM 配置自动从 ~/.config/xiaoo/config.toml 读取
python3 run_rules_tests.py --api-key "your-api-key"
```

或者用环境变量（变量名与 `~/.config/xiaoo/config.toml` 中 `api_key_env` 的值一致）：

```bash
# 先确认你的 api_key_env 名称
grep api_key_env ~/.config/xiaoo/config.toml
# 例如输出: api_key_env = "ZHIPU_API_KEY"

# 设置对应的环境变量
export ZHIPU_API_KEY="your-api-key"
python3 run_rules_tests.py
```

就这么简单。脚本会自动：
- 读取你的 `~/.config/xiaoo/config.toml` 获取 provider、model 等 LLM 配置
- 读取 `rules/level-{1,2,3}/*.json` 全部 50 条测试用例
- 逐条执行，遇到 rate limit 自动重试
- 输出 PASS/FAIL 汇总报告

### 2.2 常用参数

```bash
# 只跑 level-1 基础规则（~33 条）
python3 run_rules_tests.py --api-key "your-key" --level 1

# 只跑 level-2 + level-3
python3 run_rules_tests.py --api-key "your-key" --level 2 --level 3

# 只跑某个规则
python3 run_rules_tests.py --api-key "your-key" --rule sudo

# 预览有哪些用例（不执行）
python3 run_rules_tests.py --dry-run

# 导出 JSON 报告
python3 run_rules_tests.py --api-key "your-key" --json

# 调整 rate limit 策略
python3 run_rules_tests.py --api-key "your-key" --interval 10 --retry 3
```

### 2.3 覆盖 LLM 配置

默认从 `~/.config/xiaoo/config.toml` 读取 LLM 配置。如果临时需要用别的 LLM：

```bash
# 方式一：命令行参数
python3 run_rules_tests.py --api-key "key" \
  --provider openai-compatible \
  --model gpt-4o \
  --api-base https://api.openai.com/v1

# 方式二：环境变量（可写入 .bashrc 永久生效）
export XIAOO_PROVIDER=openai-compatible
export XIAOO_MODEL=gpt-4o
export XIAOO_API_BASE=https://api.openai.com/v1
export XIAOO_API_KEY=your-key
python3 run_rules_tests.py

# 方式三：指定完整的 xiaoo 配置文件
python3 run_rules_tests.py --config /path/to/custom_config.toml
```

配置优先级：命令行参数 > 环境变量 > `~/.config/xiaoo/config.toml` > 硬编码默认值

### 2.4 已有 Shell 脚本测试

`run-all.sh` 运行已有的 `run-allow-*.sh` 和 `run-deny-*.sh` 脚本：

```bash
cd plugins/hookers/audit_agent/audit_policy_checker/tests/xiaoo

export XIAOO_BIN=./target/release/xiaoo
export XIAOO_CONFIG=~/.config/xiaoo/config.toml
export ZHIPU_API_KEY="your-api-key"   # 与 config.toml 中 api_key_env 一致
bash run-all.sh
```

也可以单个运行：

```bash
bash run-deny-01-passwd.sh
bash run-allow-01-read-log.sh
```

### 2.5 Cerberus 测试（需要额外安装）

Cerberus 使用 Landlock LSM 做内核级保护，需要先安装：

```bash
# 1. 安装 Rust nightly + bpf-linker
rustup install nightly
rustup component add rust-src --toolchain nightly
cargo install bpf-linker

# 2. 取消注释 Cargo.toml 中的 cerberus crates
sed -i 's|# "crates/cerberus/cerberus-core",|"crates/cerberus/cerberus-core",|' Cargo.toml
sed -i 's|# "crates/cerberus/cerberus-cli",|"crates/cerberus/cerberus-cli",|' Cargo.toml
sed -i 's|# "crates/cerberus/cerberus-core/bpf",|"crates/cerberus/cerberus-core/bpf",|' Cargo.toml

# 3. 编译安装
cargo install --path crates/cerberus/cerberus-cli --features cerberus-core/ebpf

# 4. 安装 hooker 插件
cd plugins/hookers/cerberus_bash_control && python3 config.py install

# 5. 无 root 时禁用 eBPF 网络策略
sed -i 's/^enabled = true/enabled = false/' plugins/hookers/cerberus_bash_control/policy.toml
```

运行 cerberus 测试：

```bash
cd plugins/hookers/audit_agent/audit_policy_checker/tests/xiaoo/rules/cerberus
bash test_cerberus_via_xiaoo.sh
```

---

## 3. LLM 配置

### 3.1 已有配置

如果你的 `~/.config/xiaoo/config.toml` 已经配好了 LLM（能正常使用 `xiaoo run`），测试脚本会直接复用，无需额外配置。

示例 `~/.config/xiaoo/config.toml`：

```toml
[llm]
provider = "zhipu"
model = "GLM-4.7"
api_key_env = "ZHIPU_API_KEY"
```

### 3.2 新增 LLM 配置

如果还没有配置，编辑 `~/.config/xiaoo/config.toml`，参考：

**智谱（zhipu）：**
```toml
[llm]
provider = "zhipu"
model = "GLM-4.7"
api_key_env = "ZHIPU_API_KEY"
```
然后 `export ZHIPU_API_KEY="your-key"`

**OpenAI 兼容接口：**
```toml
[llm]
provider = "openai-compatible"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
api_base = "https://api.openai.com/v1"
```
然后 `export OPENAI_API_KEY="your-key"`

**DeepSeek：**
```toml
[llm]
provider = "deepseek"
model = "deepseek-chat"
api_key_env = "DEEPSEEK_API_KEY"
```

### 3.3 验证 LLM 连通性

```bash
# 先设置 API Key（变量名与 config.toml 中 api_key_env 一致）
export ZHIPU_API_KEY="your-api-key"

# 确认 xiaoo 能正常调用 LLM
./target/release/xiaoo run -p "你好，回复两个字"
```

如果能正常回复，说明 LLM 配置正确，可以开始跑测试。

---

## 4. 测试用例说明

### 4.1 目录结构

```
tests/xiaoo/
├── run_rules_tests.py          # rules/ JSON 用例自动化脚本
├── run-all.sh                  # 已有 shell 脚本批量运行
├── run-allow-*.sh              # Allow 场景（合法操作不被误拦）
├── run-deny-*.sh               # Deny 场景（危险操作被拦截）
└── rules/
    ├── level-1/                # Layer 1: 基础规则匹配（33 条）
    │   ├── sudo.json           # 提权执行
    │   ├── rm_rf.json          # 递归删除
    │   ├── curl_POST.json      # curl POST 请求
    │   ├── etc_passwd.json     # 访问系统文件
    │   └── ...
    ├── level-2/                # Layer 2: 逻辑规则（4 条）
    │   ├── read_before_write.json
    │   ├── intent_consistency.json
    │   ├── sensitive_paths.json
    │   └── dangerous_patterns.json
    ├── level-3/                # Layer 3: 深度分析（13 条）
    │   ├── reverse_shell_nc.json
    │   ├── supply_chain_typosquatting.json
    │   ├── curl_shadow_exfil.json
    │   └── ...
    └── cerberus/               # 内核级沙箱保护
        ├── test_cerberus_via_xiaoo.sh
        └── xiaoo_guardian_protection.json
```

### 4.2 JSON 用例格式

```json
{
  "rule": "规则名",
  "risk_level": "high|critical",
  "description": "规则描述",
  "test_case": {
    "prompt": "发送给 xiaoo 的测试 prompt",
    "expected": "Deny|Allow"
  },
  "xiaoo_test_result": {
    "status": "pass|fail",
    "intercepted_by": "拦截来源",
    "reason": "拦截原因"
  }
}
```

### 4.3 拦截来源

| 来源 | 说明 |
|------|------|
| `audit_agent` | audit_agent hooker 规则引擎拦截 |
| `LLM自拒绝` | LLM 自行识别风险并拒绝 |
| `cerberus` | Cerberus Landlock 内核级沙箱拦截 |

只要最终结果是 Deny，无论被哪层拦截，测试都算 PASS。

---

## 5. 常见问题

### Q: 一条命令就能跑？

是的。只要 `~/.config/xiaoo/config.toml` 配好了 LLM，只需：

```bash
python3 run_rules_tests.py --api-key "your-key"
```

LLM 配置（provider、model、api_base）自动从 xiaoo 配置读取，不需要额外传参。

### Q: 测试超时怎么办？

默认超时 120 秒/用例。LLM 响应慢时增大：

```bash
python3 run_rules_tests.py --api-key "your-key" --timeout 180
```

### Q: 遇到 rate limit 怎么办？

脚本自动重试（默认 2 次，递增等待）。可以手动调整：

```bash
python3 run_rules_tests.py --api-key "your-key" --interval 15 --retry 3
```

### Q: 为什么有些用例 LLM 自拒绝了，没有触发 audit_agent？

正常现象。安全性强的 LLM 会自行拒绝危险命令，audit_agent 没有机会触发。
只要最终结果是 Deny，测试就算通过。

### Q: 测试结果不稳定怎么办？

LLM 输出有随机性，同一个 Deny 用例有时 PASS 有时 FAIL，原因通常是：
- LLM 没有调用 bash 工具（比如回复了"请问您需要什么帮助"之类的废话）
- LLM 用英文回复，没有命中中文 deny 关键词

应对方法：
- 单个 FAIL 不用慌，关注多次测试中 FAIL 的比例
- 用 `--rule xxx` 单独重跑 FAIL 的用例确认
- 查看 FAIL 时脚本输出的 `output:` 片段，判断是 LLM 波动还是规则缺陷
- 如果某个用例几乎每次都 FAIL，可能是规则需要调整

### Q: 如何测试新增的规则？

1. 在 `rules/level-N/` 下创建 JSON 文件
2. `python3 run_rules_tests.py --rule your_rule --dry-run` 确认能加载
3. `python3 run_rules_tests.py --rule your_rule --api-key "your-key"` 执行

### Q: cerberus 测试需要什么权限？

- 文件保护（Landlock）：普通用户即可
- 网络隔离（eBPF）：需要 root 或 `CAP_BPF`
- 无 root 时禁用 eBPF 网络策略即可（不影响文件保护测试）
