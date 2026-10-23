# Secrets 存储方案设计

> **状态：已实现，但与本设计文档的若干描述不符。**
>
> 加密存储本身是真实可用的：`apps/vault`（InstanceKey / SDF）与
> `apps/shared/src/llm_secrets.rs` 均已落地，并通过
> `xiaoo-daemon config vault|set-secret|delete-secret` 暴露。
> 本文件中的**设计意图**予以保留，但凡与代码不一致处已按实际实现更正并标注
> 「已知缺口」，全部结论以 `path:line` 为据。
>
> 主要偏差：`vault.enabled` 不是开关（§2.3）；§1.2 的「按需解密、用完即销毁」不成立（§2.2）；
> 早期版本描述的 `SecretProvider` / `gateway/decrypted_api_keys.rs` 集成层**不存在**（§4.2、§6.1）。
>
> **安全演进（修复 Issue #132）**：`use_sdf=false` 的软件加密已从「源码常量派生的
> 白盒密钥」改为**每实例随机主密钥**（§3.1、§3.2）。旧 v1 格式文件在首次启动时
> 自动迁移到 v2；白盒派生逻辑仅保留用于读取旧文件，不得用于新加密。

## 目录

1. [概述](#1-概述)
2. [整体设计方案](#2-整体设计方案)
3. [详细设计方案](#3-详细设计方案)
4. [文件结构](#4-文件结构)
5. [部署与测试](#5-部署与测试)
6. [关键 API](#6-关键-api)
7. [已知限制](#7-已知限制)

---

## 1. 概述

### 1.1 目标

实现一套**本地加密**的 Secrets 存储方案，用于安全存储 API Keys 和 Verification Tokens。

### 1.2 设计原则

- **本地存储**：不依赖外部服务，所有数据存储在本地文件系统
- **加密保护**：Secrets 加密后存储，防止泄露
- **按需解密**: 发起 LLM 请求时从加密文件解密获取 API key，请求完成后立即销毁，不在内存中长期驻留
- **多密钥支持**：支持 InstanceKey（每实例随机主密钥）、TEE/SDF、HSM 三种密钥提供方式

### 1.3 支持的密钥类型

| 密钥类型 | 说明 | 加密方式 | 适用环境 |
|---------|------|---------|---------|
| **InstanceKey** | 每实例随机主密钥（`llm_secrets.key`，0600） | AES-256-GCM | 通用（软件方案） |
| **TEE/SDF** | 国密硬件模块 | SDF 国密接口 | 生产环境（高安全需求） |
| **HSM** | 硬件安全模块 | PKCS#11 | 生产环境（高安全需求） |

---

## 2. 整体设计方案

### 2.1 配置项

```toml
[vault]
use_sdf = false  # false=InstanceKey+AES-GCM, true=SDF国密
```

> ⚠️ **`enabled` 不是开关（截至 master `a58d79f`）**：`[vault] enabled` 在运行路径中
> **从未被读取**，保存与注入 Secrets 都是无条件执行的。详见 §2.2 与 §5.2.3 的说明。

### 2.2 Secrets 的实际行为（不依赖 `enabled`）

#### 数据保存方法

保存通过两条路径发生：

1. **首次启动自动保存**：daemon / TUI / CLI `run` 的启动路径统一经
   `init_llm_secrets()`（`apps/shared/src/llm_secrets.rs:194-207`）完成
   「先保存后注入」，该函数在注入环境变量**之前**调用
   `auto_save_from_env()`（`apps/serverside/src/main.rs:545`、
   `apps/endside/src/main.rs:81`、`apps/endside/src/cli/entry.rs:224`）——读
   `config.toml` 的 `llm.api_key_env`，若同名环境变量已设且非空、且 store 中
   **尚无该条目**，则加密写入 `llm_secrets.json`（仅首次保存语义）。
   - 顺序不能反：注入会把 store 里的密钥写进同一批变量名，先注入就等于把
     「环境里已有的 key」当成「已经保存过」，首次保存被永久跳过。
   - 优先级：一旦 store 已有该条目（经 `set-secret` 或 TUI 弹窗显式保存），
     残留的环境变量**不会**回滚已轮换的密钥，自动保存跳过并记 debug 日志。
     配置缺失、无 `api_key_env`、环境变量未设或为空时同样静默跳过（不视为错误）。
2. **显式 CLI 调用**：
   - **`config set-secret --environment <name>`**：从 stdin 读取密钥 → 加密 → 写入 `llm_secrets.json`
     （`apps/serverside/src/main.rs:430-452`）。
   - **`config delete-secret --environment <name>`**：从文件中删除该条目
     （`apps/serverside/src/main.rs:453-467`）。

#### 写入的并发与原子性

- 所有读改写路径（自动保存 / `set-secret` / `delete-secret`）持有同目录下的
  `flock` 互斥锁（`.llm_secrets.json.lock`，`apps/shared/src/llm_secrets.rs:56-91`），
  「存在性检查 + 写入」在同一个锁临界区内完成，跨进程并发不会丢失更新；
  锁随进程退出由内核释放，崩溃不会死锁。
- 落盘为原子写：先写同目录临时文件再 `rename` 覆盖
  （`apps/shared/src/llm_secrets.rs:235-259`），进程崩溃不会留下半写的
  `llm_secrets.json`（撕裂文件会让后续每次启动解密失败）。只读路径
  （注入 / 查询 / inspect）不加锁，靠 rename 原子性读到旧或新的完整文件。

#### 数据存储位置

```
{config_dir}/
└── llm_secrets.json    # 加密后的 Secrets 数据
```

路径由 `llm_secrets_path()` 决定：`config.toml` 所在目录下的 `llm_secrets.json`
（`apps/shared/src/llm_secrets.rs:7,342-349`）。同目录还会有一个
`.llm_secrets.json.lock`（仅作 `flock` 句柄，内容恒为空）。

#### 数据格式

```json
{
  "api_keys": {
    "OPENROUTER_API_KEY": "sk-or-v1-xxx..."
  },
  "tokens": {
    "FEISHU_VERIFICATION_TOKEN": "xxx"
  }
}
```

文件内容为加密后的二进制数据（见附录「加密格式」）。

#### 数据获取方式

**进程启动时一次性注入环境变量**，而非「按需解密、用完即销毁」：

- `inject_llm_secrets_into_env()` 解密整个 store，并把每个条目 `set_var` 到进程环境
  （`apps/shared/src/llm_secrets.rs:368-378`）。三条启动路径均经
  `init_llm_secrets()`（先自动保存、后注入）调用它：Daemon 在 runtime 组装前
  （`apps/serverside/src/main.rs:545`），TUI 在启动时调用（`apps/endside/src/main.rs:81`），
  CLI `run` 在组装配置前调用（`apps/endside/src/cli/entry.rs:224`）。
- `get_llm_secret()` 用于单次查询：先查 store，再回退到进程环境变量
  （`apps/shared/src/llm_secrets.rs:351-366`）。

> **已知缺口**：文档 §1.2 所述的「请求完成后立即销毁，不在内存中长期驻留」并不成立 ——
> 注入使用 `std::env::set_var`，密钥在整个进程生命周期内都留在环境变量中。

### 2.3 `vault.enabled` 的真实作用

`vault.enabled` 在运行路径中**不被读取**：`save_llm_secret()` / `delete_llm_secret()` /
`inject_llm_secrets_into_env()` 都只依据 `use_sdf` 决定加密方式
（`apps/shared/src/llm_secrets.rs:93-132,368-378`）。整个代码库中唯一的读取点是
`config vault` 报告的展示字段（`apps/serverside/src/vault_management.rs:74`）。

因此 `enabled = false` **不会**关闭加密存储，`enabled = true` 也**不会**开启它 ——
保存由首次启动自动保存与显式 CLI 共同驱动。

### 2.4 对比总结

| 配置 | 数据保存 | 数据获取 | 安全性 |
|------|---------|---------|--------|
| `use_sdf=false` | 首次启动自动保存 / 经 `set-secret` 加密保存到 `llm_secrets.json`（InstanceKey+AES-GCM） | 启动时解密注入环境变量 | 中（随机主密钥，需保护 `llm_secrets.key`） |
| `use_sdf=true` | 首次启动自动保存 / 经 `set-secret` 加密保存到 `llm_secrets.json`（SDF 国密） | 启动时解密注入环境变量 | 高（需鲲鹏+SDF） |
| `enabled=true/false` | **无影响**（该字段在运行路径中未被读取） | **无影响** | — |

---

## 3. 详细设计方案

### 3.1 InstanceKey + AES-GCM 流程（v2 格式，默认）

#### 加密流程

```
┌─────────────────────────────────────────────────────────────────────┐
│                  InstanceKey + AES-GCM 加密流程                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. InstanceKeyProvider.load_or_create()                            │
│     │                                                                │
│     │  读取 .llm_secrets.json.key（32B 随机密钥 + 32B HMAC 自校验）；  │
│     │  首次使用时经 OS CSPRNG 生成并以 0600 原子创建（O_EXCL 防竞态）   │
│     ▼                                                                │
│  2. 生成 12 字节随机 nonce                                          │
│  3. AES-256-GCM 加密                                               │
│     │                                                                │
│     │  plaintext → [0x02 + key_tag(16) + nonce(12) + ciphertext]    │
│     ▼                                                                │
│  4. 原子写入 llm_secrets.json（temp + rename，0600）                 │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

`key_tag` 是主密钥对固定 label 的 HMAC-SHA256 截断值：解密时先比对 tag，
密钥不匹配立即报「master key 不一致」而非笼统的解密失败。

#### 解密流程（按需获取）

```
┌─────────────────────────────────────────────────────────────────────┐
│                  InstanceKey + AES-GCM 解密流程                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. 读取 llm_secrets.json                                           │
│  2. 解析: 0x02 + key_tag(16) + nonce(12) + ciphertext               │
│  3. InstanceKeyProvider.load()（校验密钥文件 HMAC 自校验值）          │
│  4. 比对 key_tag → 不匹配即报错（master key 丢失/不匹配）            │
│  5. AES-256-GCM 解密                                                │
│  6. 注入环境变量供 LLM 请求使用                                      │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

#### v1 → v2 迁移

`init_llm_secrets()` 启动时检测 version 字节为 `0x01` 的旧文件：在 store 锁内
用公开的源码派生密钥解密（仅此一处使用，见 §3.2），随即以 v2 格式原子重写。
任何一次 `set-secret` / 自动保存也会自然写出 v2。

### 3.2 密钥文件与遗留 v1 白盒派生

#### 3.2.1 每实例主密钥（当前实现）

- **文件**: `llm_secrets.json` 同目录下的 `.llm_secrets.json.key`
- **内容**: 32 字节随机密钥 + 32 字节 HMAC-SHA256 自校验 tag（共 64 字节）
- **权限**: `0600`，`O_EXCL` 创建防止并发双写
- **实现**: `apps/vault/src/instance_key.rs`

> ⚠️ **备份提示**: `.llm_secrets.json.key` 丢失后 v2 密文**不可恢复**。备份配置
> 目录时必须连同该文件一起备份；密钥与密文同机存储，防的是「仅拿到源码/密文
> 副本」的离线解密，不防「拿到整个用户目录」的攻击者——后者请使用 SDF/HSM。

#### 3.2.2 遗留 v1 白盒派生（只读，仅供迁移）

> ⚠️ **安全警告**: v1 白盒密钥由源码常量确定性派生，任何拿到源码的人都能算出，
> 因此 v1 密文只有混淆作用、没有保密性。该派生逻辑（`legacy_v1_master_key()`，
> `apps/vault/src/whitebox.rs`）**仅用于解密旧文件以完成 v2 迁移**，已从
> `WhiteBoxKeyProvider` 收敛为自由函数，不再实现 `KeyProvider`，任何新加密
> 均不得使用。

### 3.3 TEE/SDF 国密流程

#### 加密流程

```
┌─────────────────────────────────────────────────────────────────────┐
│                         SDF 国密加密流程                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. init_sdf_provider("/usr/local/sdf/lib/libsdf.so")              │
│     │                                                                │
│     │  加载 SDF 动态库                                               │
│     ▼                                                                │
│  2. SDF_OpenDevice() → device_handle                               │
│  3. SDF_OpenSession() → session_handle                            │
│  4. SDF_GetKEKAccessRight() → 获取 KEK 权限                        │
│  5. SDF_GenerateKeyWithKEK() → 生成会话密钥并用 KEK 加密导出        │
│  6. SDF_Encrypt() → 使用会话密钥进行国密加密                        │
│  7. 写入 llm_secrets.json (包含加密后的会话密钥 + 密文)             │
│  8. SDF_ReleaseKEKAccessRight() → 释放权限                         │
│  9. SDF_CloseSession()                                             │
│  10. SDF_CloseDevice()                                             │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

#### 解密流程（按需获取，用完即销毁）

```
┌─────────────────────────────────────────────────────────────────────┐
│                         SDF 国密解密流程                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. 读取 llm_secrets.json                                           │
│  2. init_sdf_provider("/usr/local/sdf/lib/libsdf.so")              │
│  3. SDF_OpenDevice() → device_handle                               │
│  4. SDF_OpenSession() → session_handle                            │
│  5. SDF_GetKEKAccessRight() → 获取 KEK 权限                        │
│  6. SDF_ImportKeyWithKEK() → 导入加密的会话密钥                     │
│  7. SDF_Decrypt() → 使用会话密钥进行国密解密                        │
│  8. 发起 LLM 请求时按需获取密钥                                      │
│  9. 请求完成后立即销毁（不驻留内存）                                  │
│  10. SDF_ReleaseKEKAccessRight() → 释放权限                         │
│  11. SDF_CloseSession()                                            │
│  12. SDF_CloseDevice()                                             │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.4 密钥获取机制（按实现更正）

**不存在**「按需解密 provider」这一层。早期版本在此处给出的
`get_decrypted_api_key(env_name)` 与 `init_secret_provider(secrets_path, use_sdf)`
在本仓库中**没有实现**（§4.2、§6.1 的证据）。

实际机制是**启动时一次性解密并注入进程环境变量**：

```rust
// apps/shared/src/llm_secrets.rs:368-378
pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()> {
    let store = load_secrets_store(&llm_secrets_path(config_path), use_sdf)?;
    for (environment, secret) in store.api_keys.into_iter().chain(store.tokens) {
        std::env::set_var(environment, secret);
    }
    Ok(())
}
```

调用点：三条启动路径均经组合函数 `init_llm_secrets()`
（`apps/shared/src/llm_secrets.rs:194-207`，内部固定「先 `auto_save_from_env()`
后注入」的顺序）——`apps/serverside/src/main.rs:545`（daemon）、
`apps/endside/src/main.rs:81`（TUI）与 `apps/endside/src/cli/entry.rs:224`（CLI `run`）。

**特点（实际）**：
- 启动时解密整个 store，一次 `set_var` 完成注入
- 之后程序通过环境变量读取，**密钥在进程生命周期内一直驻留**
- 单次查询可用 `get_llm_secret()`，它先查 store、再回退环境变量
  （`apps/shared/src/llm_secrets.rs:351-366`）

> **已知缺口**：与 §1.2 宣称的「请求完成后立即销毁，不在内存中长期驻留」不符 ——
> 注入使用 `std::env::set_var`，进程退出前不会清除。

### 3.5 API Key 解析优先级

```
1. HTTP 请求 / CLI 参数中的 api_key 覆盖（仅 Daemon HTTP API 的 llm.api_key 字段或 CLI 的 --api-key 参数）
        ↓
2. llm_secrets.json (按需解密获取，通过 api_key_env 解析)
        ↓
3. 环境变量 (fallback，通过 api_key_env 解析)
```

> **Note**: `[llm]` 配置段中没有 `api_key` 直接配置字段。API key 只能通过 `api_key_env` 指向环境变量或加密文件，或通过 HTTP 请求 / CLI 参数临时覆盖。

---

## 4. 文件结构

### 4.1 apps/vault

密钥提供者核心库。

```
apps/vault/src/
├── lib.rs                          # 主入口，导出所有 public API
├── key_provider.rs                 # KeyProvider trait 定义
├── types.rs                        # KeyMaterial
├── key_provider_error.rs           # KeyProviderError
├── instance_key.rs                 # InstanceKeyProvider（每实例随机主密钥）
├── whitebox.rs                     # 遗留 v1 白盒派生（仅解密旧文件供迁移）
├── sdf.rs                          # SDF 国密接口封装 + TeeKeyProvider
└── hsm.rs                          # HSM PKCS#11 接口（预留）
```

### 4.2 apps/shared (Secrets 存储)

Secrets 存储与解密逻辑在 `xiaoo-shared` crate 中，CLI/TUI/Daemon 共用。

```
apps/shared/src/
└── llm_secrets.rs                  # 本地加密存储管理
                                    # - save_llm_secret() / delete_llm_secret()
                                    # - auto_save_from_env()（仅首次保存）
                                    # - init_llm_secrets()（启动路径组合入口：先保存后注入）
                                    # - get_llm_secret()
                                    # - inject_llm_secrets_into_env()
                                    # - inspect_secret_store()
                                    # - llm_secrets_path()
                                    # - StoreLock（flock 互斥）/ 原子落盘（temp+rename）
                                    # - encrypt_aes_gcm() / decrypt_aes_gcm()
```

> 早期版本的文档在此处描述了一个 `gateway/decrypted_api_keys.rs` 模块，导出
> `SecretProvider`、`init_secret_provider()`、`get_decrypted_api_key()`。**该模块与这些
> 符号均不存在**（`git grep -n 'decrypted_api_keys\|SecretProvider\|init_secret_provider\|get_decrypted_api_key' -- '*.rs'`
> 无匹配）。真实的公开 API 就是上表列出的函数
> （`apps/shared/src/llm_secrets.rs:93-391`）。

### 4.3 apps/endside (TUI 配置入口)

```
apps/endside/src/
├── main.rs                         # TUI 启动入口，经 config::init_llm_secrets() 完成保存+注入
└── support/
    └── config.rs                   # TUI 配置集成，重新导出 save_llm_secret /
                                    # init_llm_secrets
```

`apps/endside/src/main.rs:81` 调用 `config::init_llm_secrets()`；
`apps/endside/src/support/config.rs:489-495` 是这两个函数的薄转发。CLI `run`
同样经 `init_llm_secrets()` 完成保存+注入（`apps/endside/src/cli/entry.rs:224-231`）。

### 4.4 各文件作用

| 文件 | 作用 |
|------|------|
| `apps/vault/src/instance_key.rs` | 每实例随机主密钥的生成、加载与 HMAC 自校验 |
| `apps/vault/src/whitebox.rs` | 遗留 v1 白盒密钥派生（仅解密旧文件供迁移） |
| `apps/vault/src/sdf.rs` | SDF 国密接口封装 |
| `apps/vault/src/hsm.rs` | HSM PKCS#11 接口（预留） |
| `apps/shared/src/llm_secrets.rs` | 加密/解密、加密文件读写、环境变量注入 |
| `apps/serverside/src/vault_management.rs` | `config vault` / `set-secret` / `delete-secret` 的引用模型与报告 |

---

## 5. 部署与测试

### 5.1 部署视图

```
┌─────────────────────────────────────────────────────────────────────┐
│                        部署视图                                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                      xiaoo 进程                              │    │
│  │  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐  │    │
│  │  │    xiaoo     │  │ xiaoo --cli  │  │ xiaoo-daemon │  │    │
│  │  └───────────────┘  └───────────────┘  └───────────────┘  │    │
│  │           │                │                  │          │    │
│  │           └────────────────┼──────────────────┘          │    │
│  │                          │                              │    │
│  │              ┌───────────▼───────────┐                │    │
│  │              │   llm_secrets.rs      │                │    │
│  │              │ - encrypt_aes_gcm()   │                │    │
│  │              │ - decrypt_aes_gcm()   │                │    │
│  │              └───────────┬───────────┘                │    │
│  │                          │                              │    │
│  │              ┌───────────▼───────────┐                │    │
│  │              │  vault (instance/sdf) │                │    │
│  │              └───────────────────────┘                │    │
│  │                          │                              │    │
│  │              ┌───────────▼───────────┐                │    │
│  │              │  llm_secrets.json      │                │    │
│  │              │  (加密文件)            │                │    │
│  │              └───────────────────────┘                │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 5.2 配置文件

#### 5.2.1 [vault] 配置项

```toml
[vault]
enabled = false  # 是否启用加密存储
use_sdf = false  # 加密方式
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `enabled` | bool | `false` | 是否启用加密存储到 `llm_secrets.json` |
| `use_sdf` | bool | `false` | false=InstanceKey(每实例随机主密钥)，true=SDF国密(仅鲲鹏服务器) |

#### 5.2.2 配置文件示例

**vault.enabled = true (加密存储 - InstanceKey)**

```toml
[vault]
enabled = true
use_sdf = false  # 仅测试环境使用

[llm]
provider = "openrouter"
model = "anthropic/claude-sonnet-4"   # required when [llm.profiles] is absent
api_key_env = "OPENROUTER_API_KEY"
```

> ⚠️ **密钥文件提示**: `use_sdf=false` 使用每实例随机主密钥（`.llm_secrets.json.key`）。
> 备份配置目录时必须连同该文件一起备份，丢失后密文不可恢复；对「整个用户目录被拿走」
> 的威胁模型请使用 `use_sdf=true`。

**vault.enabled = true (加密存储 - SDF 国密)**

```toml
[vault]
enabled = true
use_sdf = true  # 仅适用于鲲鹏服务器
```

> ⚠️ **SDF 国密要求**: `use_sdf=true` 仅支持**鲲鹏系列服务器**，并需要部署 SDF 国密模块。详细方法请参考：
> - [鲲鹏商密应用使用指南](https://www.hikunpeng.com/document/detail/zh/kunpengcctrustzone/cca/twp/Kunpeng_ommercialcryptography_19_0002.html)

**vault.enabled = false (不加密存储)**

```toml
[vault]
enabled = false  # 不保存 Secrets，使用环境变量

[llm]
provider = "openrouter"
model = "anthropic/claude-sonnet-4"   # required when [llm.profiles] is absent
api_key_env = "OPENROUTER_API_KEY"
```

#### 5.2.3 配置行为说明

`enabled` 字段在运行路径中不被读取（§2.3），因此下表只列 `use_sdf` 的效果：

| 配置 | Secrets 保存 | 启动时行为 |
|------|-------------|-----------|
| 任意 `enabled` 取值 | **无影响** —— 首次启动自动保存与显式 `config set-secret` 均可写入文件 | 若 `llm_secrets.json` 存在，解密后注入进程环境变量 |
| `use_sdf=false` | `set-secret` / 自动保存用 InstanceKey+AES-GCM 加密 | 用每实例主密钥解密 |
| `use_sdf=true` | `set-secret` / 自动保存用 SDF 国密加密 | 用 SDF 密钥解密（仅鲲鹏） |

真实的加密方式选择来自 `[vault] use_sdf`，由 `get_use_sdf_from_config()` 读取
（`apps/shared/src/llm_secrets.rs:23-39`）。

### 5.3 环境变量

| 环境变量 | 说明 | 默认值 |
|---------|------|--------|
| `USE_SDF` | 使用 SDF 国密 | `false` |
| `LD_LIBRARY_PATH` | SDF 动态库路径 | `/usr/local/sdf/lib` |

### 5.4 SDF 国密部署要求

> ⚠️ **重要**: SDF 国密 (`use_sdf=true`) **仅支持鲲鹏系列服务器**。
>
> 详细部署方法请参考：[鲲鹏商密应用使用指南](https://www.hikunpeng.com/document/detail/zh/kunpengcctrustzone/cca/twp/Kunpeng_ommercialcryptography_19_0002.html)

**前置条件**:
1. 服务器必须是鲲鹏系列 ( Kunpeng ARM64 )
2. 需要部署 SDF 国密模块 (`libsdf.so`) 及依赖库
3. 需要在 config.toml 中设置 `use_sdf = true`

### 5.5 测试方法

#### 编译

```bash
# 默认编译 (InstanceKey + AES-GCM)
cargo build --release -p xiaoo-endside --bin xiaoo

# 启用 SDF 国密 (仅鲲鹏服务器)
cargo build --release -p xiaoo-endside --bin xiaoo --features tee_sdf
```

#### 单元测试

```bash
cargo test --package vault
cargo test --package xiaoo-shared
```

#### 功能验证

```bash
# 设置环境变量
export OPENROUTER_API_KEY='sk-or-v1-xxx'
export USE_SDF=false

# 运行 TUI（config.toml 设置了 llm.api_key_env 且同名环境变量非空、store 中
# 尚无该条目时，首次启动自动保存，与 vault.enabled 无关）
./target/release/xiaoo --config config.toml

# 检查加密文件
hexdump -C ~/.xiaoo/config/llm_secrets.json | head
```

---

## 6. 关键 API

### 6.1 llm_secrets 模块（真实公开 API）

以下为 `apps/shared/src/llm_secrets.rs` 中实际存在的公开函数：

```rust
// 写入 / 删除单个密钥（按 environment 变量名索引；写路径持锁 + 原子落盘）
pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()>;   // :93-106
pub fn delete_llm_secret(config_path: &Path, env_name: &str) -> Result<bool>;            // :108-132

// 首次启动自动落盘（读 llm.api_key_env 指向的环境变量；store 已有该条目时跳过）
pub fn auto_save_from_env(config_path: &Path) -> Result<()>;                              // :134-191

// 启动路径组合入口：先 auto_save_from_env() 再 inject_llm_secrets_into_env()，
// 顺序在函数内部固化，调用方无法写反
pub fn init_llm_secrets(config_path: &Path) -> Result<()>;                                // :194-207

// 解析单个密钥（先查 store，再回退进程环境变量）
pub fn get_llm_secret(config_path: &Path, env_name: &str) -> Result<String>;             // :351-366

// 解密整个 store 并注入进程环境变量（经 init_llm_secrets 使用）
pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()>;                    // :368-378

// 只返回密钥名称，不暴露值
pub fn inspect_secret_store(config_path: &Path) -> Result<SecretStoreMetadata>;          // :380-391

// 存储路径
pub fn llm_secrets_path(config_path: &Path) -> PathBuf;                                  // :342-349
```

> **不存在**：`load_llm_secrets_to_memory()` 与 `save_token()` 从未在本仓库中定义
> （`git grep -n 'load_llm_secrets_to_memory\|save_token' -- '*.rs'` 无匹配）。

### 6.2 CLI / 参考面（此前文档缺失）

Secrets 的唯一入口是 daemon 的 `config` 子命令。全部经 `xiaoo-daemon` 提供：

| 命令 | 行为 | 位置 |
|------|------|------|
| `xiaoo-daemon config vault [--config <path>]` | 打印 `VaultReport` JSON：provider、是否可用、以及每个 `*_env` 配置项的解析状态 | `apps/serverside/src/main.rs:421-429` |
| `xiaoo-daemon config set-secret --environment <name> [--config <path>] < secret` | 从 **stdin** 读取密钥（去除尾部换行），校验该 environment 被配置引用后加密写入 | `apps/serverside/src/main.rs:430-452` |
| `xiaoo-daemon config delete-secret --environment <name> [--config <path>]` | 删除条目，输出 `{"schema_version":1,"removed":<bool>}` | `apps/serverside/src/main.rs:453-467` |

命令的用法文本见 `apps/serverside/src/main.rs:1376-1377`。

`set-secret` / `delete-secret` 会先调用 `ensure_environment_is_referenced()`：environment
必须被某个配置文件字段引用（例如 `llm.api_key_env`、`mcp_server.agent.bearer_token_env`、
`server.operation_backend.options.api_key_env`），否则命令报错
（`apps/serverside/src/vault_management.rs:92-268`）。

> **没有引用语法**：不存在 `vault://` 或 `ref:` 形式的取值语法 ——
> `git grep -n 'vault://\|ref:' -- '*.rs'` 无匹配。配置中一律写**环境变量名**，
> 运行时由 `inject_llm_secrets_into_env()` 把对应值放进环境（§2.2）。

`config vault` 报告中的 `status` 还包含 `inline` 分类：若配置里把
`api_key` / `bearer_token` / `webhook_secret_token` / `verification_token`
直接写成了字面值，会被标记为内联密钥
(`apps/serverside/src/vault_management.rs:261-267`)。

### 6.3 密钥提供者

```rust
// InstanceKey（每实例随机主密钥）
pub struct InstanceKeyProvider { ... }
impl KeyProvider for InstanceKeyProvider { ... }

// 遗留 v1 白盒派生（仅解密旧文件供迁移，不实现 KeyProvider）
pub fn legacy_v1_master_key() -> Result<[u8; 32], KeyProviderError>;

// SDF 国密
pub fn init_sdf_provider(path: &str) -> Result<()>;

// TEE 密钥提供者
pub struct TeeKeyProvider { ... }
impl KeyProvider for TeeKeyProvider { ... }

// HSM (预留)
pub struct HsmKeyProvider { ... }
impl KeyProvider for HsmKeyProvider { ... }
```

> 注：早期文档在此处列出 `encrypt_secret()` / `decrypt_secret()` 作为 SDF 的公开函数。
> 实际的 SDF 加解密接口位于 `apps/vault/src/sdf.rs` 的
> `sdf_encrypt()`（`:336`）与 `sdf_decrypt()`（`:478`）。

---

## 7. 已知限制

### 7.1 InstanceKey

- 主密钥与密文同机同目录存储，防「源码 + 密文副本」的离线解密，不防拿到整个用户目录的攻击者；后者请使用 SDF/HSM
- `.llm_secrets.json.key` 丢失后 v2 密文不可恢复，必须随配置目录一起备份
- 旧 v1 文件在迁移完成前仍可被持有源码者解密；启动或首次保存即完成迁移

### 7.2 SDF 国密

- 需要 libsdf.so 及依赖库
- KEK 口令默认为 NULL，需要配置真实 KEK
- libcrypto 版本需与 libsdf.so 匹配

### 7.4 实现缺口（截至 master `a58d79f`）

- **`vault.enabled` 是死配置**：运行路径从不读取，保存/注入无条件执行（§2.3）。
- **密钥常驻进程环境**：`inject_llm_secrets_into_env()` 用 `std::env::set_var` 注入
  （`apps/shared/src/llm_secrets.rs:368-378`），与 §1.2 宣称的「用完即销毁」不符。
- **无 `vault://` / `ref:` 引用语法**：配置中只能引用环境变量名（§6.2）。

---

## 附录：加密格式

### InstanceKey + AES-GCM（v2，当前格式）

```
┌──────────┬──────────────┬──────────────┬──────────────────┐
│ Version  │  Key Tag     │  Nonce       │  Ciphertext      │
│  1 byte  │  16 bytes    │  12 bytes    │  (含 auth tag)    │
│   0x02   │  HMAC 截断   │   random     │  AES-256-GCM     │
└──────────┴──────────────┴──────────────┴──────────────────┘
```

Key Tag 为主密钥对固定 label 的 HMAC-SHA256 截断值，用于解密前显式校验
master key 匹配（`apps/vault/src/instance_key.rs` 的 `store_key_tag()`）。

### 遗留 v1 白盒格式（只读，仅供迁移）

```
┌─────────────────────────────────────────────────────────────┐
│  Version (1 byte)  │  Nonce (12 bytes)  │  Ciphertext     │
│         1          │     random          │  (含 auth tag)   │
└─────────────────────────────────────────────────────────────┘
```

密钥由源码常量派生（`legacy_v1_master_key()`），无保密性；启动迁移或下次
保存后即转为 v2。

### SDF 国密

```
┌────────────────┬──────────────────┬─────────────────┬──────────────────┐
│  version (1B)  │  key_length (4B) │  key_buffer     │  ciphertext      │
│       1        │   big-endian u32 │  key_length 字节 │  (SMS4-ECB 结果) │
└────────────────┴──────────────────┴─────────────────┴──────────────────┘
```

布局由 `sdf_encrypt()` 写出：`vec![1u8]` + `key_length.to_be_bytes()` +
`key_buffer[..key_length]` + ciphertext
（`apps/vault/src/sdf.rs:465-470`）。最小头部为 5 字节
（`ENCRYPTED_HEADER_SIZE = ENCRYPTED_VERSION_SIZE + ENCRYPTED_KEY_LENGTH_SIZE`，
`apps/vault/src/sdf.rs:291`），`sdf_decrypt()` 按同样顺序解析
（`apps/vault/src/sdf.rs:478-520`）。

> **修正**：早期文档把此处写成 `Version(1) | IV(16) | Ciphertext`。**SDF 路径不存在 IV** ——
> 加密使用 SMS4-ECB（无 IV 概念），头部的第二个字段是变长的 `key_buffer` 长度 + 内容，
> 而不是 16 字节随机 IV。
