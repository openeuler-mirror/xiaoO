# Cerberus 运行环境能力矩阵

这份文档回答四个实际问题：

1. **能否编译**：当前环境 / build feature 是否能产出对应功能
2. **能否运行**：策略在这个运行时上会成功执行、降级执行，还是直接 fail-closed
3. **配置差别**：哪些配置在不同环境上会变成关键开关
4. **安全防护差异**：最终到底保留了哪些保护，丢了哪些保护

本文只描述当前代码已经实现的行为，不扩展“未来也许会支持”的语义。

---

## 先看总原则

Cerberus 的环境差异主要发生在 **Linux sandbox 层**，不是整个执行管线。

- `cerberus-core` / `cerberus-cli` 本身可以在更广泛环境上编译
- 但 **Landlock、seccomp、mount isolation、Linux namespaces、eBPF network policy** 都是环境敏感能力
- 某些能力缺失时会 **fail-closed**
- 某些能力缺失时只会 **degraded**
- 还有少数情况下会按 policy 明确允许 **继续执行但失去某类隔离**

不要把 `profile show` 里的 capability 展示理解成“绝对可用证明”。当前 capability 探测对 seccomp 和 mount fallback 偏乐观，真正执行时仍可能失败。

---

## 一、编译维度：能否编译出哪些能力

### 1. 默认构建（不带 `ebpf` feature）

可以编译：

- `cerberus-core`
- `cerberus-cli`
- Landlock / seccomp / namespace / mount fallback 相关 Linux sandbox 代码

不能编译进来的能力：

- eBPF network monitor / enforce backend
- 真实的 `network_policy` matcher + enforcer 路径

这意味着：

- 普通 `namespaces.network = false` 的“禁网”能力仍然存在
- 但启用 `network_policy.enabled = true` 时，默认构建会 **fail-closed**

### 2. `--features cerberus-core/ebpf` 构建

额外编译进来：

- eBPF loader
- eBPF audit backend
- network matcher / monitor / enforce 路径

这意味着：

- `network_policy.enabled = true` 才有机会进入真实 backend
- 但是否真的能运行，还取决于运行时 eBPF 初始化是否成功

---

## 二、运行维度：在什么环境上会怎样

## 1. 非 Linux

### 能否运行

- Cerberus 本身可以运行
- 但 Linux sandbox 能力整体视为 **unsupported**

### 配置差别

- `landlock_optional` / `mount_isolation_fallback` 不会把非 Linux 变成 Linux sandbox
- `namespaces.*` 不会获得真实 Linux namespace 隔离
- `network_policy` 没有 Linux eBPF 后端可走

### 安全防护差异

保留：

- 普通执行流程
- 非 sandbox 逻辑（例如请求构造、结果渲染、历史记录）

不保留：

- Landlock 文件系统隔离
- seccomp syscall 过滤
- Linux namespace 隔离
- mount isolation fallback
- eBPF network policy

### 结论

**非 Linux 不应被理解为“弱化版 Linux sandbox”，而是“没有 Linux sandbox”。**

---

## 2. Linux + Landlock 可用 + 请求的关键 namespace 可用

这是 Cerberus 的主目标环境。

### 能否运行

- 可以运行
- 若 policy 没有其他冲突，通常能达到 `enforced` 或接近 `enforced`

### 配置差别

- `landlock_optional` / `mount_isolation_fallback` 在这类环境里通常不影响主路径，因为 Landlock 已可用
- `network_policy` 是否可用，仍取决于是否带 `ebpf` feature

### 安全防护差异

保留：

- Landlock 文件系统隔离
- namespace 隔离（取决于 policy 请求项）
- resource limits
- seccomp 过滤

如果同时带 `ebpf` 且 backend 初始化成功，还保留：

- 真实 `network_policy` monitor / enforce

### 结论

这是“Cerberus 安全模型最接近设计预期”的环境。

---

## 3. Linux + Landlock 不可用 + `landlock_optional = false` + `mount_isolation_fallback = false`

### 能否运行

- **不能正常继续执行**
- 对需要文件系统隔离的 policy，会 **fail-closed**

### 配置差别

这是最严格的文件系统隔离配置：

- 不允许失去 Landlock
- 不允许退化到 mount fallback

### 安全防护差异

保留：

- fail-closed 行为本身

不保留：

- 命令执行结果，因为执行会被阻止

### 结论

这种配置表达的是：**没有 Landlock，就宁可不跑。**

---

## 4. Linux + Landlock 不可用 + `landlock_optional = true` + `mount_isolation_fallback = true`

### 能否运行

- 可能继续运行
- 会走 mount isolation fallback
- enforcement level 应按 **degraded** 理解

### 配置差别

这是“允许 Landlock 缺失，但仍尽量保留文件系统可见性收缩”的配置。

### 安全防护差异

仍保留：

- “只把允许路径 bind-mount 到新 root” 的可见性缩减思路
- 资源限制
- 其他可用的 namespace / seccomp 路径

会丢失 / 退化：

- **精确文件权限语义** 不再等价于 Landlock
- `readonly` 与 `readexecute` 在 mount fallback 下等价
- `readwrite` 与 `readwriteexecute` 在 mount fallback 下等价
- execute 维度不会被独立精确表达
- `/dev`、`/proc`、`/sys` 这类伪文件系统不会通过 bind mount 保留下来

### 结论

mount fallback 是 **可见性/只读性级别的降级保护**，不是 Landlock 的等价替身。

---

## 5. Linux + Landlock 不可用 + `landlock_optional = true` + `mount_isolation_fallback = false`

### 能否运行

- 可以继续运行
- 但会打印警告并 **不做文件系统隔离**

### 配置差别

这是最容易被误解的一档配置：

- 你允许 Cerberus 在没有 Landlock 时继续
- 但又禁止 mount fallback
- 最终结果就是：**继续执行，但没有 fs isolation**

### 安全防护差异

保留：

- 其他不依赖 Landlock 的保护（如果还能成功应用）

失去：

- 文件系统隔离本身

### 结论

这不是“弱一点的文件系统隔离”，而是 **没有文件系统隔离**。

---

## 6. Linux + 缺失关键 namespace 能力

当前代码里，namespace 不是一刀切；有些缺失是 **critical**，有些只算 **degraded**。

### critical：会 fail-closed

下面这些如果被 policy 请求，但运行时不可用，会直接失败：

- `user`
- `pid`
- `network`（仅当 policy 需要“禁网隔离”时）

### degraded：不作为前置 fail-closed 的只有 mount

- `mount`

但这里要注意：

**`mount` 只是在 precheck 里被归类为 degraded，不代表后续 setup 一定能顺利继续。**

### 安全防护差异

如果 critical namespace 缺失：

- Cerberus 直接阻止执行
- 不会假装“隔离差一点也能跑”

如果只是 mount namespace 不理想：

- capability / enforcement 展示会更接近 degraded
- 但真正运行时仍可能因为后续 namespace / mount setup 失败而报错

### 结论

**user / pid / network(block) 缺失是硬失败；mount 缺失更像“先报 degraded，再看后续 setup 是否真能活”。**

---

## 7. Linux + 默认构建（无 `ebpf`）+ `network_policy.enabled = true`

### 能否运行

- **不能正常执行**
- 会在 runtime requirement / sandbox setup 阶段直接 fail-closed

### 配置差别

这里最重要的配置不是 `landlock_optional`，而是：

- `namespaces.network` 是否为 `true`
- `network_policy.enabled` 是否为 `true`
- `network_policy.mode` 是 `monitor` 还是 `enforce`

### 安全防护差异

保留：

- fail-closed

不保留：

- 真实 network policy backend，因为根本没编进来

### 结论

默认构建下：

- `network = false` 的“禁网 namespace 语义”可以工作
- 但 `network_policy.enabled = true` 不会被静默降级成 display-only，而是 **直接失败**

---

## 8. Linux + `ebpf` 构建 + eBPF backend 初始化成功

### 能否运行

- 可以运行
- `network_policy` 会进入真实 backend

### 配置差别

- `network_policy.enabled = true` 才会启用
- `namespaces.network` 必须是 `true`
- `mode = "monitor"` 与 `mode = "enforce"` 差异真实存在

### 安全防护差异

保留：

- network event monitor
- matcher / policy evaluation
- `enforce` 模式下的 kill path

但要注意：

- 这不是 “完全没有网络能力” 的语义
- 它更像“观察到违规网络操作后再处理”
- 不能把它和 `namespaces.network = false` 混成同一种安全保证

### 结论

`network_policy` 是 eBPF monitor / enforce 机制；`namespaces.network = false` 是 namespace 级禁网。两者不是一回事。

---

## 9. Linux + `ebpf` 构建 + eBPF backend 初始化失败

### 能否运行

- 对启用了 `network_policy` 的执行，会失败

### 配置差别

- 只要你真的启用了 `network_policy`
- 就不允许“编译时带了 ebpf，但运行时 backend 起不来，还继续偷偷执行”

### 安全防护差异

保留：

- fail-closed

### 结论

**eBPF 路径不是“尽力而为的软特性”，而是启用后要求后端真实可用。**

---

## 三、最重要的配置开关，到底影响什么

## 1. `landlock_optional`

- `false`：Landlock 缺失时更偏向 fail-closed
- `true`：允许继续走降级路径

它只影响 **Landlock 缺失时的处理**，不解决 namespace / eBPF 的硬缺失。

## 2. `mount_isolation_fallback`

- `true`：Landlock 缺失时，尝试 mount fallback
- `false`：不尝试 mount fallback

它只对 **Landlock -> mount** 的降级有意义。

## 3. `namespaces.network`

- `true`：允许联网
- `false`：要求禁网隔离

它和 `network_policy` 不同。前者是 namespace 级 allow/block，后者是 eBPF 级 monitor/enforce。

## 4. `network_policy.enabled`

- `false`：只存储 / 展示，不强制执行
- `true`：必须满足 backend 条件，否则 fail-closed

---

## 四、按环境快速看结果

| 环境 | 能否编译 | 能否运行 | 配置重点 | 安全防护结果 |
|---|---|---|---|---|
| 非 Linux | 能 | 能，但无 Linux sandbox | Linux sandbox 配置不会真正生效 | 无 Landlock / seccomp / namespaces / mount fallback / eBPF |
| Linux + Landlock + 关键 namespaces + 默认构建 | 能 | 能 | `network_policy.enabled` 不能开 | 有 Landlock / namespaces / seccomp / RLIMIT；无 eBPF network policy |
| Linux + Landlock + 关键 namespaces + ebpf 构建 | 能 | 能 | `network_policy.enabled` 可真实使用 | 有 Landlock / namespaces / seccomp / RLIMIT / eBPF network policy |
| Linux + 无 Landlock + fallback 全关 | 能 | 不能（fs rules 需要隔离时） | `landlock_optional=false`, `mount_isolation_fallback=false` | fail-closed |
| Linux + 无 Landlock + 允许 mount fallback | 能 | 能，degraded | `landlock_optional=true`, `mount_isolation_fallback=true` | 有可见性收缩，但 fs 权限语义退化，execute 语义不精确 |
| Linux + 无 Landlock + 允许继续但不 fallback | 能 | 能 | `landlock_optional=true`, `mount_isolation_fallback=false` | 无 fs isolation |
| Linux + 默认构建 + `network_policy.enabled=true` | 能 | 不能 | 默认构建无 eBPF backend | fail-closed |
| Linux + ebpf 构建 + `network_policy.enabled=true` + backend 初始化失败 | 能 | 不能 | backend 必须真实可用 | fail-closed |
| Linux + 缺 user/pid/network-block namespace | 能 | 不能 | 这些是 critical namespace | fail-closed |

---

## 五、最容易误解的几点

1. **`degraded` 不等于“一定能安全继续跑”**
   - 有些 degraded 只是 capability 展示层面的结果
   - 真实 setup 仍可能在后面失败

2. **mount fallback 不是 Landlock 等价替代**
   - 它保留的是“可见性缩减”思路
   - 不是精确 permission backend

3. **`network = false` 不等于 `network_policy`**
   - 前者是 namespace 级禁网
   - 后者是 eBPF monitor/enforce

4. **默认构建不是“弱化 network_policy”，而是“启用后直接 fail-closed”**

5. **`profile show` 很有用，但不是最终证明**
   - 它能帮助你理解当前 runtime 的大方向
   - 但不要把它当成内核级 end-to-end 保证报告

---

## 六、建议怎么用

### 如果你要最强、最可预测的防护

- Linux
- Landlock 可用
- 关键 namespace 可用
- `landlock_optional = false`
- `mount_isolation_fallback = false`
- 需要 `network_policy` 时用 `ebpf` 构建

### 如果你要“尽量能跑，但仍保留一部分隔离”

- Linux
- `landlock_optional = true`
- `mount_isolation_fallback = true`

但你必须接受：

- fs permission 语义会退化
- execute 维度不再精确

### 如果你在非 Linux 上运行

应把 Cerberus 理解为：

- 还能做命令执行与策略对象处理
- 但不能提供 Linux sandbox 级别的安全保证
