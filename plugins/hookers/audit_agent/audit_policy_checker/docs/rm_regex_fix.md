# rm 命令正则规则修复说明

## 背景

审计 Agent 的 Layer 1.1（启发式静态检测）中，`rm` 命令的正则规则存在严重 bug，导致大量正常操作被误判为 critical 级别拦截。

## 问题描述

### Bug 1：flag 分组可选，匹配过宽

**原规则（heuristic_detector.py 第 24 行）：**

```python
r"\brm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)?(-[a-zA-Z]*r[a-zA-Z]*\s+)?/"
```

两个 flag 分组都是 `?`（可选），实际匹配逻辑变成：

```
rm + 空格 + [可选-f] + [可选-r] + /
```

即 **只要 `rm` 后跟以 `/` 开头的绝对路径，不管有没有 `-rf` flag，都命中**。

**误报示例：**

| 命令 | 匹配结果 | 期望结果 |
|------|---------|---------|
| `rm /tmp/test.sh` | critical（误报） | 放行 |
| `rm ./build/output.o` | 不命中（相对路径） | 放行 |
| `rm /home/user/old_script.sh` | critical（误报） | 放行 |
| `rm -f /var/log/test.log` | critical（过严） | medium |

### Bug 2：reason 信息误导

命中后返回的 reason 是 `"检测到递归强制删除关键路径 (rm -rf /...)"`，但实际命令 `rm /tmp/test.sh` 根本没有 `-rf`，用户看到告警信息与实际命令不符。

### Bug 3：第二条规则被短路，成为死代码

**原规则（heuristic_detector.py 第 30 行）：**

```python
r"\brm\s+-[a-zA-Z]*r[a-zA-Z]*[a-zA-Z]*f[a-zA-Z]*\s+"
```

这条规则意图匹配 `rm -rf`，但由于 Bug 1 的规则在前且短路返回，这条规则永远不会执行。同时它还要求 `-r` 在 `-f` 前面，`rm -fr` 不会命中。

## 修复方案

### 核心思路

- **rm 命令的风险 = flag 危险度 + 目标路径敏感度**，两者缺一不可
- **无 flag 的 `rm` 删除普通文件**（如 `rm /tmp/test.sh`）是正常操作，应放行交给下层判断
- **`rm -rf` 删除关键系统目录**是最高危操作，必须 critical 拦截
- 规则按严格到宽松排序，利用短路机制优先匹配高危场景

### 新规则（5 条）

替换 `CRITICAL_COMMAND_PATTERNS` 中原有的 2 条 rm 规则：

```python
# rm 命令分级检测（规则从严格到宽松，先匹配的优先）
# 1. rm -rf 根目录 — critical（最高危）
{
    "pattern": r"\brm\s+-[a-zA-Z]*r[a-zA-Z]*f[a-zA-Z]*\s+/\s",
    "risk_level": "critical",
    "risk_type": "script_execution",
    "reason": "检测到递归强制删除根目录 (rm -rf /)",
},
# 2. rm 同时带 -r 和 -f 且目标是绝对路径 — critical
{
    "pattern": r"\brm\s+-(?=[^\s]*r)(?=[^\s]*f)[a-zA-Z]*\s+/",
    "risk_level": "critical",
    "risk_type": "script_execution",
    "reason": "检测到递归强制删除绝对路径 (rm -rf /...)",
},
# 3. rm 带 -r 或 -f 且目标是关键系统目录 — critical
{
    "pattern": r"\brm\s+-[a-zA-Z]*[rf][a-zA-Z]*\s+/(etc|var|home|usr|boot|root|opt|srv|sys|proc)\b",
    "risk_level": "critical",
    "risk_type": "script_execution",
    "reason": "检测到删除关键系统目录",
},
# 4. rm -r 后跟绝对路径（无 -f）— high
{
    "pattern": r"\brm\s+-[a-zA-Z]*r[a-zA-Z]*\s+/",
    "risk_level": "high",
    "risk_type": "script_execution",
    "reason": "检测到递归删除操作 (rm -r /...)",
},
# 5. rm -f 后跟绝对路径（无 -r）— medium
{
    "pattern": r"\brm\s+-[a-zA-Z]*f[a-zA-Z]*\s+/",
    "risk_level": "medium",
    "risk_type": "script_execution",
    "reason": "检测到强制删除文件 (rm -f /...)",
},
```

### 正则说明

**规则 1**：`\brm\s+-[a-zA-Z]*r[a-zA-Z]*f[a-zA-Z]*\s+/\s`
- 要求 `-r` 在 `-f` 前面，且路径恰好是 `/`（根目录），后面有空白（行尾或空格）
- 匹配：`rm -rf /`、`rm -rf / `

**规则 2**：`\brm\s+-(?=[^\s]*r)(?=[^\s]*f)[a-zA-Z]*\s+/`
- 用两个 lookahead 断言 flag 中同时包含 `r` 和 `f`（顺序不限）
- 匹配：`rm -rf /tmp/`、`rm -fr /etc/passwd`

**规则 3**：`\brm\s+-[a-zA-Z]*[rf][a-zA-Z]*\s+/(etc|var|home|usr|boot|root|opt|srv|sys|proc)\b`
- 只要有 `-r` 或 `-f` 中的任意一个，且目标是关键系统目录
- 匹配：`rm -r /var/log/`、`rm -f /etc/config`

**规则 4**：`\brm\s+-[a-zA-Z]*r[a-zA-Z]*\s+/`
- 只有 `-r`（无 `-f`），绝对路径
- 匹配：`rm -r /tmp/test/`

**规则 5**：`\brm\s+-[a-zA-Z]*f[a-zA-Z]*\s+/`
- 只有 `-f`（无 `-r`），绝对路径
- 匹配：`rm -f /tmp/test.log`

### 匹配结果对比

| 命令 | 修复前 | 修复后 | 说明 |
|------|--------|--------|------|
| `rm /tmp/test.sh` | **critical** | **放行** | 无 flag，正常操作 |
| `rm ./build/output.o` | 不命中 | **放行** | 相对路径，正常操作 |
| `rm /home/user/old_script.sh` | **critical** | **放行** | 无 flag，交下层判断 |
| `rm -rf /` | critical | **critical** | 根目录删除，最高危 |
| `rm -rf /etc/` | critical | **critical** | 关键系统目录 |
| `rm -fr /etc/` | critical | **critical** | -fr 顺序也匹配 |
| `rm -r /var/log/` | critical | **critical** | 关键系统目录 |
| `rm -f /var/log/test.log` | critical | **critical** | 关键系统目录 |
| `rm -rf /tmp/test.sh` | critical | **critical** | 有 -rf |
| `rm -r /tmp/test.sh` | critical | **high** | 只有 -r，非关键目录 |
| `rm -f /tmp/test.sh` | critical | **medium** | 只有 -f，非关键目录 |

### 设计原则：为什么不需要安全路径白名单

没命中 Layer 1.1 的命令不会直接放行，还会经过：

- **Layer 1.2（逻辑规则）**：基于意图一致性判断
- **Layer 1.3（LLM 深度分析）**：语义级风险评估

所以 `rm /tmp/test.sh` 在 1.1 放行后，仍会被 1.2/1.3 检查。如果在 1.1 加白名单模式（默认拒绝），会导致大量合理操作被拦截，维护成本高。分层架构下每层各司其职即可。

## 修改步骤

### 1. 找到文件

```
<项目根路径>/audit_policy_checker/security/heuristic_detector.py
```

### 2. 定位 CRITICAL_COMMAND_PATTERNS 列表

在文件开头找到 `CRITICAL_COMMAND_PATTERNS: list[dict] = [` 开头的列表。

### 3. 删除旧规则

删除列表中的前 2 条 rm 相关规则（搜索 `"rm"` 或 `"递归强制删除"`）：

```python
# 删除这 2 条
{
    "pattern": r"\brm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)?(-[a-zA-Z]*r[a-zA-Z]*\s+)?/",
    ...
},
{
    "pattern": r"\brm\s+-[a-zA-Z]*r[a-zA-Z]*[a-zA-Z]*f[a-zA-Z]*\s+",
    ...
},
```

### 4. 插入新规则

在原位置插入上述 5 条新规则，保持在 `CRITICAL_COMMAND_PATTERNS` 列表的最前面。

### 5. 重新安装依赖

```bash
cd audit_policy_checker
source venv/bin/activate
pip install -e . -q
```

### 6. 验证

运行以下 Python 脚本确认匹配结果：

```python
import re

patterns = [
    (r'\brm\s+-[a-zA-Z]*r[a-zA-Z]*f[a-zA-Z]*\s+/\s',                                    'critical', 'rm -rf 根目录'),
    (r'\brm\s+-(?=[^\s]*r)(?=[^\s]*f)[a-zA-Z]*\s+/',                                     'critical', 'rm -rf 绝对路径'),
    (r'\brm\s+-[a-zA-Z]*[rf][a-zA-Z]*\s+/(etc|var|home|usr|boot|root|opt|srv|sys|proc)\b','critical', '关键系统目录'),
    (r'\brm\s+-[a-zA-Z]*r[a-zA-Z]*\s+/',                                                 'high',     'rm -r 绝对路径'),
    (r'\brm\s+-[a-zA-Z]*f[a-zA-Z]*\s+/',                                                 'medium',   'rm -f 绝对路径'),
]

tests = [
    ('rm /tmp/test.sh',           '不命中'),
    ('rm -r /tmp/test.sh',        'high'),
    ('rm -f /tmp/test.sh',        'medium'),
    ('rm -rf /tmp/test.sh',       'critical'),
    ('rm -rf /etc/',              'critical'),
    ('rm -rf /',                  'critical'),
    ('rm ./build/output.o',       '不命中'),
    ('rm /home/user/old_script.sh','不命中'),
]

print(f'{"命令":<40} {"期望":<12} {"实际":<12} {"结果"}')
print('-' * 90)
all_pass = True
for cmd, expected in tests:
    actual = '不命中'
    for pat, level, desc in patterns:
        if re.search(pat, cmd, re.IGNORECASE):
            actual = level
            break
    ok = '✓' if actual == expected else '✗'
    if actual != expected:
        all_pass = False
    print(f'{cmd:<40} {expected:<12} {actual:<12} {ok}')

print()
print('ALL PASS' if all_pass else 'SOME FAILED')
```

预期输出应全部为 `✓`。

## 注意事项

- 此修复只涉及 **Layer 1.1 的 `CommandPatternScanner` 子检测器**
- **`UserRuleMatcher`**（user_rules.json）中的 `"危险 shell 命令"` 规则对英文命令字符串无效（它匹配的是中文 category 名，不是原始命令），不影响本次修复
- 如果其他仓库有类似的 rm 正则规则，应采用相同的分级思路修复，不要简单地把 flag 分组设为可选
