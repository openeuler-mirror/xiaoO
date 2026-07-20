# L2 敏感路径规则审定 — 移除误报路径 + 修正策略 + 补充覆盖缺口

## 背景

基于 `hkl.log` 误报修复（`docs/l2_false_positive_fix.md`）和产品终审，对 `SENSITIVE_PATHS` 规则表做二次审定：移除误报率极高的设备路径、修正策略标注、补充覆盖缺口、新增 `read_only` 策略维度。

## 策略维度（三种）

| 策略字段 | 含义 | 拦截读 | 拦截写/删 | 典型路径 |
|---------|------|--------|-----------|---------|
| `credential=True` | 凭据/密钥类，读即泄密 | ✓ | ✓ | `/etc/shadow`、`/dev/mem`、`/etc/ssh/ssh_host_*_key` |
| `read_only=True` | 仅拦截读取，写/删放行 | ✓ | ✗ | `/dev/random` |
| 默认（无标记） | 仅拦截写/删，读放行 | ✗ | ✓ | `/sys/`、`/etc/hosts`、`~/.bashrc` |

检测逻辑在 `_check_sensitive_path_access` 里分三个分支：

```python
if is_credential:
    # 读写均拦
    return LogicRuleResult(hit=True, ...)
if is_read_only:
    if is_write_op:
        continue  # 写操作放行（如投喂熵池）
    return LogicRuleResult(hit=True, ...)  # 读操作拦截
if not is_write_op:
    continue  # 默认：读放行
return LogicRuleResult(hit=True, ...)  # 默认：写/删拦截
```

## 第一类：移除（4 条）

| 路径 | 原策略 | 裁定 | 理据 |
|------|--------|------|------|
| `/dev/null` | 仅拦截写入 | **移除** | `2>/dev/null`/`&>/dev/null` 是丢弃 stderr 的 Shell 范式，误报率接近 100%；写入无安全风险 |
| `/dev/zero` | 仅拦截写入 | **移除** | `dd if=/dev/zero` 是创建空文件/磁盘填充的标准操作，写入无安全风险 |
| `/dev/random` | 仅拦截写入 | **改为 read_only=True** | 读取可耗尽熵池导致 TLS 阻塞（攻击向量）；写入是投喂熵（增强安全） |
| `/dev/urandom` | 仅拦截写入 | **移除** | 非阻塞式设备，读取不耗尽熵池、写入是投喂熵，无论读写均无安全威胁 |

**移除生效机制**：源码删除后，`_merge_rule_categories` 将用户副本中残留的内置规则标记为 `enabled=False`（禁用但保留，dashboard 可见），不是暴力删除——用户还能在 dashboard 上看到这些规则（标注为已禁用），还能手动启用。

## 第二类：策略修正（2 条）

| 路径 | 原策略 | 裁定 | 理据 |
|------|--------|------|------|
| `/dev/mem` | 仅拦截写入（默认） | **credential=True（读写均拦）** | 读 `/dev/mem` 可 dump 物理内存中的明文密码（mimipenguin 原理），比写入同等危险 |
| `.ssh/authorized_keys` | credential（读写均拦） | **保留 credential** | 公钥虽可公开，但在 Agent 高权限场景下，读取暴露了"谁能无密码登录"的完整清单，对攻击者是有价值的侦察信息 |

**未采纳的建议**：
- `/etc/sudoers` 改为仅拦截写入 → 未采纳，sudoers 暴露提权拓扑
- `.ssh/authorized_keys` 改为仅拦截写入 → 未采纳，Agent 高权限场景下读取有侦察价值
- 非 root 读 shadow 改为 warn → 未采纳，Agent 不区分 root/非 root，实现复杂
- 内容指纹白名单 → 未采纳，超出敏感路径规则职责，L1/L3 已有类似能力

## 第三类：覆盖缺口补充（4 组）

| 新路径 | 策略 | risk_level | 理据 |
|--------|------|-----------|------|
| `~/.bashrc` | 仅拦截写入 | high | 最经典的命令劫持持久化路径（alias sudo=...、植入反 Shell） |
| `~/.bash_profile` | 仅拦截写入 | high | 同上，登录触发 |
| `~/.profile` | 仅拦截写入 | high | 同上 |
| `/etc/cron.d/` | 仅拦截写入 | high | 现代 Linux 倾向拆分定时任务到此目录，绕过 `/etc/crontab` |
| `/var/spool/cron/` | 仅拦截写入 | high | 用户级 crontab 写入位置，绕过 `/etc/crontab` |
| `/etc/ssh/ssh_host_rsa_key` | credential | critical | 主机私钥泄露可 MITM 劫持所有 SSH 连接 |
| `/etc/ssh/ssh_host_ed25519_key` | credential | critical | 同上 |
| `/etc/ssh/ssh_host_ecdsa_key` | credential | critical | 同上 |
| `/etc/pam.d/` | 仅拦截写入 | high | 篡改 PAM 可绕过系统认证 |

## 第四类：工程化改进 — 内置规则移除同步

`_merge_rule_categories` 新增逻辑：源码中不存在的内置规则，在用户副本中自动标记为 `enabled=False`（而非暴力删除），保留审计可见性：

```python
source_ids = {sr.get("id") for sr in source_rules if sr.get("id")}
for ur in user_rules:
    if ur.get("id") and ur.get("builtin") and ur["id"] not in source_ids and ur.get("enabled", True):
        ur["enabled"] = False
```

## 涉及文件

| 文件 | 改动 |
|------|------|
| `audit_policy_checker/security/logic_rules.py` | `SENSITIVE_PATHS` 表重写（移除4条、修正2条、新增9条、加 `read_only` 字段）；`_check_sensitive_path_access` 新增 `is_read_only` 分支 |
| `audit_policy_checker/runtime_config.py` | `_l2_sensitive_paths_to_rules` 加 `read_only` 字段；`get_enabled_l2_sensitive_paths` 透传 `read_only`；merge 同步 `read_only` 字段 + 移除内置规则 |
| `SECURITY_RULES.md` | 策略维度说明（三类+read_only+移除说明）；Critical/High/Medium 表重写 |
| `audit_dashboard/static/index.html` | 新增 `.read-only-badge` CSS；规则列表和搜索结果渲染 `read_only` 徽标 |

## 行为验证

```python
# read_only: /dev/random
assert hit("dd if=/dev/random of=/dev/null") is False  # 写放行
assert hit("cat /dev/random") is True                  # 读拦截

# credential: /dev/mem
assert hit("cat /dev/mem") is True                    # 读写均拦

# 移除: /dev/null / /dev/zero / /dev/urandom
assert hit("cat /dev/null") is False                  # 不在规则表
assert hit("dd if=/dev/zero of=/tmp/file") is False

# 新增: shell profile (仅拦截写入)
assert hit("cat ~/.bashrc") is False                  # 读放行
assert hit("echo alias >> ~/.bashrc") is True         # 写拦截

# 新增: cron.d / SSH host key / PAM
assert hit("echo job > /etc/cron.d/x") is True
assert hit("cat /etc/ssh/ssh_host_rsa_key") is True   # credential
assert hit("echo恶意 > /etc/pam.d/x") is True
```

17 条验证全 PASS。
