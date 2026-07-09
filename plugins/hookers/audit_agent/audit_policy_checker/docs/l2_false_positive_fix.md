# L2 误报修复：sysfs 只读访问与 blockdev 2>/dev/null

## 背景

源自 `hkl.log` 的三条 L2 误报（均为 Deny）：

| # | 命令 | 误判规则 | 误判原因 |
|---|------|---------|---------|
| 1 | `file_read /sys/class/block/sda/size` | sensitive_path_access | sysfs 规则一刀切拦截，不区分读写 |
| 2 | `cat /sys/class/block/sda/size` | sensitive_path_access | 同上 |
| 3 | `blockdev --getsize /dev/sda 2>/dev/null` | read_before_write | `2>/dev/null` 被当成重定向写入，blockdev 不在只读白名单 |

这三条都是只读的设备信息查询操作，应放行。

## 问题描述

### Bug 1：敏感路径不区分读写（误报 1/2）

`_check_sensitive_path_access` 的放行逻辑只对 `~` 开头的保护目录区分读写，`/sys/`、`/proc/sys/`、`/dev/*` 等非 `~` 路径**无论读写全部拦截**。`cat /sys/...`（只读）被一刀切 Deny。

函数注释写"读取操作放行"，但实际只对家目录路径生效。

### Bug 2：重定向写判定误判 /dev/null（误报 3）

`_check_read_before_write` 的重定向检测：
- `2>/dev/null` 里有 `>` → 触发重定向写入检测
- `blockdev` 不在只读命令白名单（原白名单只有 `cat/head/tail/grep/...` 传统读命令）→ `is_write = True`
- 提取命令参数 `/dev/sda` 当写入目标，history 无前置读 → Deny

根因：`2>/dev/null` 是丢弃 stderr 的标准 shell 写法，不是写文件意图；且只读命令白名单不含 `blockdev`/`lsblk`/`smartctl` 等只读系统工具。

### Bug 3：写操作判定不识别 rm/cp/dd（潜在漏报）

`WRITE_KEYWORDS` 走子串匹配，无法可靠识别 `rm`（"rm" 会误命中 `arm`/`form`），所以 `rm /boot`、`dd of=/dev/sda` 等写删命令不会被 `is_write_op` 识别。修复 Bug 1（按读写区分）后，这会导致**写敏感路径被当成只读放行**——漏报。

## 修复方案

### 1. 抽出统一的写操作判定 `_is_write_operation`

新增模块级函数，供 `_check_read_before_write` 与 `_check_sensitive_path_access` 共用，避免两处逻辑漂移：

```python
def _is_write_operation(action_type, action_detail) -> bool:
    # 1) 写入关键词（write/写入/修改/删除/...）
    if any(kw in action_type or kw in action_detail for kw in WRITE_KEYWORDS):
        return True
    # 2) 写/删命令（词边界正则，避免 rm 误命中 arm）
    if any(rx.search(action_detail) for rx in _WRITE_COMMAND_RES):
        return True
    # 3) 真实重定向写入（先排除 /dev/null 丢弃，再看首命令是否非只读）
    if ">" in action_detail:
        detail_without_devnull = _DEVNULL_REDIRECT_RE.sub("", action_detail)
        if ">" in detail_without_devnull:
            first_word = action_detail.split()[0].strip().lower() if action_detail.split() else ""
            if first_word not in READ_ONLY_COMMANDS:
                return True
    return False
```

### 2. 新增 `READ_ONLY_COMMANDS`（扩充只读命令白名单）

```python
READ_ONLY_COMMANDS = {
    # 传统文本查看/过滤
    "cat", "head", "tail", "less", "more", "grep", "find", "awk", "sed",
    "sort", "uniq", "wc", "cut", "strings", "od", "xxd", "hexdump", "tr",
    "file", "stat", "du", "df", "ls", "dir", "tree", "nl", "tac", "rev",
    # 只读系统/设备信息查询
    "lsblk", "blockdev", "smartctl", "udevadm", "dmidecode", "lscpu", "lspci",
    "lsusb", "lsmem", "lsns", "lsof", "hwinfo", "inxi", "fdisk", "parted",
    "dmesg", "journalctl", "systemctl", "hostnamectl", "localectl", "timedatectl",
    "ps", "top", "free", "vmstat", "iostat", "mpstat", "pidof", "pgrep",
    "ip", "ifconfig", "route", "ss", "netstat", "arp", "ethtool", "nmcli",
    "uname", "arch", "nproc", "getconf", "getent", "id", "whoami", "who",
    "w", "last", "uptime", "lsmod", "modinfo", "sysctl",
}
```

### 3. 新增 `_DEVNULL_REDIRECT_RE`（豁免 /dev/null 重定向）

```python
_DEVNULL_REDIRECT_RE = re.compile(r"(?:2|&|1)?\s*>\s*/dev/null\b")
```

匹配 `2>/dev/null`、`&>/dev/null`、`>/dev/null`、`2 > /dev/null` 等写法，在重定向判定前先剔除，使 `blockdev --getsize /dev/sda 2>/dev/null` 不再被判为写。

### 4. 新增 `_WRITE_COMMAND_RES`（写/删命令词边界正则）

```python
_WRITE_COMMAND_RES = [
    re.compile(r"\brm\b"),       re.compile(r"\bunlink\b"),  re.compile(r"\brmdir\b"),
    re.compile(r"\bshred\b"),    re.compile(r"\bcp\b"),      re.compile(r"\bmv\b"),
    re.compile(r"\binstall\b"),  re.compile(r"\btee\b"),     re.compile(r"\bdd\b"),
    re.compile(r"\bmkfs\b"),     re.compile(r"\b(fdisk|parted|cfdisk|sfdisk)\b"),
    re.compile(r"\bchmod\b"),    re.compile(r"\bchown\b"),   re.compile(r"\btruncate\b"),
]
```

词边界 `\b` 保证 `rm` 不误命中 `arm`/`form`，`cp` 不误命中 `scp`/`bcp`，`dd` 不误命中 `adduser`。

### 5. 敏感路径按读写区分 + credential 标记

`_check_sensitive_path_access` 改为：

```python
if path_match:
    if is_credential:           # 凭据/密钥：读写都拦
        return LogicRuleResult(hit=True, ...)
    if not is_write_op:         # 非凭据：只读放行
        continue
    return LogicRuleResult(hit=True, ...)   # 非凭据：写/删拦截
```

给"只读也必拦"的密钥/凭据路径标 `credential=True`（读它们就是泄密）：

```python
{"path": "/etc/shadow",        ..., "credential": True},
{"path": "/etc/gshadow",       ..., "credential": True},
{"path": "/etc/sudoers",       ..., "credential": True},
{"path": ".ssh/id_rsa",        ..., "credential": True},
{"path": ".ssh/id_ed25519",    ..., "credential": True},
{"path": ".ssh/authorized_keys",..., "credential": True},
```

`/etc/passwd`、`/etc/hosts`、`/etc/crontab`、`/sys/`、`/proc/sys/`、`/dev/*`、`/boot/` 等保持非 credential，只读放行、写/删拦截。

### 6. 修正 credential 路径以 `/` 开头时的 `\b` 边界失效

原代码对 `/` 开头的 credential 路径用 `\b{path}\b`，但 `/` 不是单词字符，空格→`/` 之间不构成 `\b`，导致 `cat /etc/shadow` 匹配不到。改为 `(?:^|[\s/\\]){path}\b`（与 `.` 开头的 `.env` 同一分支）：

```python
if path.startswith(".") or path.startswith("/"):
    path_match = bool(re.search(rf"(?:^|[\s/\\]){escaped}\b", action_detail))
else:
    path_match = bool(re.search(rf"\b{escaped}\b", action_detail))
```

### 7. runtime_config 合并同步 credential 字段

`_merge_rule_categories` 原本只新增缺失规则，不更新已有规则字段。给 `/etc/shadow` 等加 `credential=True` 后，老用户副本不会自动跟上。补一段：对已有规则，同步源码的 `credential` 字段（仅此一字段，不动 `enabled` 等用户开关）：

```python
else:
    ur = next((r for r in user_rules if r.get("id") == sr.get("id")), None)
    if ur is not None:
        for field, val in sr.items():
            if field in ("id", "path", "risk_level", "desc", "builtin"):
                if field not in ur:
                    ur[field] = val
            elif field == "credential":
                ur["credential"] = val   # 源码为准
```

老用户无需 reset runtime 即可生效。

## 行为对比

| 命令 | 修复前 | 修复后 | 说明 |
|------|--------|--------|------|
| `cat /sys/class/block/sda/size` | **Deny（误报）** | **Allow** | 非凭据路径只读放行 |
| `blockdev --getsize /dev/sda 2>/dev/null` | **Deny（误报）** | **Allow** | /dev/null 豁免 + blockdev 只读 |
| `lsblk -b 2>/dev/null` | Deny（误报） | **Allow** | 同上 |
| `cat /etc/shadow` | Deny | **Deny** | credential，读写均拦（不回归） |
| `echo x >> /etc/hosts` | Deny | **Deny** | 非凭据写操作拦截（不回归） |
| `rm -rf /boot/vmlinuz` | Deny | **Deny** | rm 识别为写，写敏感路径拦截（不回归） |
| `echo 1 > /sys/.../brightness` | Deny | **Deny** | 非凭据写操作拦截（不回归） |
| `dd if=/dev/zero of=/dev/sda` | Deny | **Deny** | dd 识别为写，命中 /dev/zero（不回归） |
| `ls /boot` | Deny | **Allow** | 非凭据只读放行（行为变更，合理） |
| `cat /etc/passwd` | Deny | **Allow** | 非凭据只读放行（行为变更，/etc/passwd 公开可读） |

## 涉及文件

| 文件 | 改动 |
|------|------|
| `audit_policy_checker/security/logic_rules.py` | 新增 `READ_ONLY_COMMANDS`/`_DEVNULL_REDIRECT_RE`/`_WRITE_COMMAND_RES`/`_is_write_operation`；`SENSITIVE_PATHS` 给密钥文件标 credential；`_check_read_before_write` 与 `_check_sensitive_path_access` 改用统一写判定 + 读写区分；修正 `/` 开头 credential 路径边界匹配 |
| `audit_policy_checker/runtime_config.py` | `_merge_rule_categories` 同步已有规则的 credential 字段 |
| `SECURITY_RULES.md` | 同步 read_before_write 检测范围、敏感路径读写区分说明、Critical/High 表 credential 标注、示例改为读写对比 |
| `plugins/tests/.../rules/level-2/` | 新增 `sysfs_readonly_allow.json`、`blockdev_devnull_allow.json`、`write_sensitive_path_deny.json` 端到端用例 |

## 验证

### 单元级行为验证（本地可跑）

```python
from audit_policy_checker.security.logic_rules import LogicRulesChecker, SENSITIVE_PATHS
from audit_policy_checker.runtime_config import load_runtime_config
load_runtime_config()  # 触发 merge 同步 credential
chk = LogicRulesChecker()
# 注入 /sys/ 模拟规则启用（用户可能已禁用）
chk._sensitive_paths = list(chk._sensitive_paths) + [
    next(sp for sp in SENSITIVE_PATHS if sp["path"] == "/sys/")]
def mk(d): return {"action_type": "bash", "action_detail": d}
assert chk.check("", [], mk("cat /sys/class/block/sda/size"), "").hit is False
assert chk.check("", [], mk("blockdev --getsize /dev/sda 2>/dev/null"), "").hit is False
assert chk.check("", [], mk("echo x >> /etc/hosts"), "").hit is True
assert chk.check("", [], mk("cat /etc/shadow"), "").hit is True
assert chk.check("", [], mk("rm -rf /boot/vmlinuz"), "").hit is True
```

### 端到端用例

```bash
cd plugins/tests/hookers/audit_agent/xiaoo
python3 run_rules_tests.py --rule sysfs_readonly_allow --api-key "$KEY"
python3 run_rules_tests.py --rule blockdev_devnull_allow --api-key "$KEY"
python3 run_rules_tests.py --rule write_sensitive_path_deny --api-key "$KEY"
```

**注意**：`sysfs_readonly_allow` 前提是 runtime 中 `path_sys` 规则启用；若被禁用则天然 Allow（失去回归意义）。

## 注意事项

- 此修复只涉及 **Layer 2 逻辑规则**，不动 Layer 1.1 启发式正则（`rm -rf /` 等仍由 L1.1 分级拦截，见 `docs/rm_regex_fix.md`）。
- `sed`/`awk` 在 `READ_ONLY_COMMANDS` 中按只读看待，但其 `-i` 原地编辑能改文件——原地改写由 L1.1/L3 另行覆盖，此处不重复。
- **生效需重新安装**：`install.sh` 用 `pip install .`（非 editable），venv 的 `site-packages` 是源码拷贝。改了 `logic_rules.py`/`runtime_config.py` 后必须重跑 `pip install .`（或开发期用 `pip install -e .`）才能让 dashboard / hooker 用上新逻辑，否则仍跑旧代码。
- **dashboard 规则列表自动更新**：merge 把 `credential=True` 同步进 runtime JSON 后，前端 `GET /api/rules` 照常渲染。credential 字段**不显示在规则卡片上**（卡片只显示 内置/自定义、risk、risk_type、路径、说明、开关）——即 `/etc/shadow`（credential）和 `/etc/hosts`（非 credential）在界面上看不出区别。新建规则表单有 credential 复选框，但已有规则无编辑入口。此为既有 UI 缺口，本次未改。
