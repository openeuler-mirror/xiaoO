# Secrets 存储方案设计

> **状态（截至 master `a58d79f`）：已实现，但与本设计文档的若干描述不符。**
>
> 加密存储本身是真实可用的：`apps/vault`（WhiteBox / SDF）与
> `apps/shared/src/llm_secrets.rs` 均已落地，并通过
> `xiaoo-daemon config vault|set-secret|delete-secret` 暴露。
> 本文件中的**设计意图**予以保留，但凡与代码不一致处已按实际实现更正并标注
> 「已知缺口」，全部结论以 `path:line` 为据。
>
> 主要偏差：`vault.enabled` 不是开关（§2.3）；§1.2 的「按需解密、用完即销毁」不成立（§2.2）；
> 早期版本描述的 `SecretProvider` / `gateway/decrypted_api_keys.rs` 集成层**不存在**（§4.2、§6.1）。

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
- **多密钥支持**：支持 WhiteBox、TEE/SDF、HSM 三种密钥提供方式

### 1.3 支持的密钥类型

| 密钥类型 | 说明 | 加密方式 | 适用环境 |
|---------|------|---------|---------|
| **WhiteBox** | 软件级白盒密钥 | AES-256-GCM | ⚠️ 仅测试环境 |
| **TEE/SDF** | 国密硬件模块 | SDF 国密接口 | 生产环境 |
| **HSM** | 硬件安全模块 | PKCS#11 | 生产环境 |

---

## 2. 整体设计方案

### 2.1 配置项

```toml
[vault]
use_sdf = false  # false=WhiteBox+AES-GCM, true=SDF国密
```

> ⚠️ **`enabled` 不是开关（截至 master `a58d79f`）**：`[vault] enabled` 在运行路径中
> **从未被读取**，保存与注入 Secrets 都是无条件执行的。详见 §2.2 与 §5.2.3 的说明。

### 2.2 Secrets 的实际行为（不依赖 `enabled`）

#### 数据保存方法

保存**只**通过显式 CLI 调用发生，启动时不会自动从环境变量抓取：

1. **`config set-secret --environment <name>`**：从 stdin 读取密钥 → 加密 → 写入 `llm_secrets.json`
   （`apps/serverside/src/main.rs:428-455`）。
2. **`config delete-secret --environment <name>`**：从文件中删除该条目
   （`apps/serverside/src/main.rs:456-470`）。

> **已知缺口**：`auto_save_from_env()`（`apps/shared/src/llm_secrets.rs:79-...`）实现了
> 「首次启动自动从环境变量保存」，但在整个 workspace 中**没有任何调用方**
> （`git grep -n auto_save_from_env -- '*.rs'` 只有定义处，无调用）。因此下文
> §2.2 旧版描述的「首次启动自动保存」在当前代码中**不会发生**。

#### 数据存储位置

```
{config_dir}/
└── llm_secrets.json    # 加密后的 Secrets 数据
```

路径由 `llm_secrets_path()` 决定：`config.toml` 所在目录下的 `llm_secrets.json`
（`apps/shared/src/llm_secrets.rs:6,219-224`）。

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
  （`apps/shared/src/llm_secrets.rs:245-254`）。Daemon 在 runtime 组装前调用
  （`apps/serverside/src/main.rs:530`），TUI 在启动时调用（`apps/endside/src/main.rs:76`）。
- `get_llm_secret()` 用于单次查询：先查 store，再回退到进程环境变量
  （`apps/shared/src/llm_secrets.rs:228-241`）。

> **已知缺口**：文档 §1.2 所述的「请求完成后立即销毁，不在内存中长期驻留」并不成立 ——
> 注入使用 `std::env::set_var`，密钥在整个进程生命周期内都留在环境变量中。

### 2.3 `vault.enabled` 的真实作用

`vault.enabled` 在运行路径中**不被读取**：`save_llm_secret()` / `delete_llm_secret()` /
`inject_llm_secrets_into_env()` 都只依据 `use_sdf` 决定加密方式
（`apps/shared/src/llm_secrets.rs:33-56,245-254`）。整个代码库中唯一的读取点是
`config vault` 报告的展示字段（`apps/serverside/src/vault_management.rs:74`）。

因此 `enabled = false` **不会**关闭加密存储，`enabled = true` 也**不会**开启它 ——
保存只由显式 CLI 驱动。

### 2.4 对比总结

| 配置 | 数据保存 | 数据获取 | 安全性 |
|------|---------|---------|--------|
| `use_sdf=false` | 经 `set-secret` 加密保存到 `llm_secrets.json`（WhiteBox+AES-GCM） | 启动时解密注入环境变量 | 仅测试 |
| `use_sdf=true` | 经 `set-secret` 加密保存到 `llm_secrets.json`（SDF 国密） | 启动时解密注入环境变量 | 高（需鲲鹏+SDF） |
| `enabled=true/false` | **无影响**（该字段在运行路径中未被读取） | **无影响** | — |

---

## 3. 详细设计方案

### 3.1 WhiteBox + AES-GCM 流程

#### 加密流程

```
┌─────────────────────────────────────────────────────────────────────┐
│                    WhiteBox + AES-GCM 加密流程                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. WhiteBoxKeyProvider.get_key()                                   │
│     │                                                                │
│     │  从代码碎片重建 32 字节 master key                              │
│     ▼                                                                │
│  2. 生成 12 字节随机 nonce                                          │
│  3. AES-256-GCM 加密                                               │
│     │                                                                │
│     │  plaintext → [version(1) + nonce(12) + ciphertext]            │
│     ▼                                                                │
│  4. 写入 llm_secrets.json                                           │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

#### 解密流程（按需获取，用完即销毁）

```
┌─────────────────────────────────────────────────────────────────────┐
│                    WhiteBox + AES-GCM 解密流程                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. 读取 llm_secrets.json                                           │
│  2. 解析: version(1) + nonce(12) + ciphertext                      │
│  3. WhiteBoxKeyProvider.get_key()                                   │
│     │                                                                │
│     │  从代码碎片重建 32 字节 master key                              │
│     ▼                                                                │
│  4. AES-256-GCM 解密                                               │
│  5. 发起 LLM 请求时按需获取密钥                                      │
│  6. 请求完成后立即销毁（不驻留内存）                                  │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.2 WhiteBox 密钥配置

#### 3.2.1 当前状态

> ⚠️ **安全警告**: WhiteBox 密钥当前设为 NULL（全零），**仅适用于测试环境**。
>
> **生产环境请使用 SDF 国密 (`use_sdf=true`) 或 HSM 方案。**

**文件位置**: `apps/vault/src/whitebox.rs`

**当前配置**:
```rust
const FRAG_A: [u8; 8] = [0u8; 8];  // 全零，仅测试用
const FRAG_B: [u8; 8] = [0u8; 8];  // 全零，仅测试用
const FRAG_C: [u8; 8] = [0u8; 8];  // 全零，仅测试用
const FRAG_D: [u8; 8] = [0u8; 8];  // 全零，仅测试用
```

#### 3.2.2 自定义密钥配置参考

> 仅适用于测试环境。生产环境请使用 SDF 国密方案。

如果需要在测试环境中使用自定义密钥，可通过修改代码碎片实现：

```rust
// 示例: 设置自定义密钥 "MySecretKey12345678901234567890" (32 bytes)

const FRAG_A: [u8; 8] = [
    'M' ^ 0x5A, 'y' ^ 0x5A, 'S' ^ 0x5A, 'e' ^ 0x5A,
    'c' ^ 0x5A, 'r' ^ 0x5A, 'e' ^ 0x5A, 't' ^ 0x5A,
];

const FRAG_B: [u8; 8] = [
    'K' ^ 0xA5, 'e' ^ 0xA5, 'y' ^ 0xA5, '1' ^ 0xA5,
    '2' ^ 0xA5, '3' ^ 0xA5, '4' ^ 0xA5, '5' ^ 0xA5,
];

const FRAG_C: [u8; 8] = [
    '6' ^ 0x3C, '7' ^ 0x3C, '8' ^ 0x3C, '9' ^ 0x3C,
    '0' ^ 0x3C, '1' ^ 0x3C, '2' ^ 0x3C, '3' ^ 0x3C,
];

const FRAG_D: [u8; 8] = [
    '4' ^ 0x7E, '5' ^ 0x7E, '6' ^ 0x7E, '7' ^ 0x7E,
    '8' ^ 0x7E, '9' ^ 0x7E, '0' ^ 0x7E, '1' ^ 0x7E,
];
```

**密钥生成方法**:

```bash
# 生成随机 32 字节密钥
openssl rand -hex 32

# 或使用 Python
python3 -c "import secrets; print(secrets.token_hex(32))"
```

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
// apps/shared/src/llm_secrets.rs:245-254
pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()> {
    let store = load_secrets_store(&llm_secrets_path(config_path), use_sdf)?;
    for (environment, secret) in store.api_keys.into_iter().chain(store.tokens) {
        std::env::set_var(environment, secret);
    }
    Ok(())
}
```

调用点：`apps/serverside/src/main.rs:530`（daemon）与
`apps/endside/src/main.rs:76`（TUI）。

**特点（实际）**：
- 启动时解密整个 store，一次 `set_var` 完成注入
- 之后程序通过环境变量读取，**密钥在进程生命周期内一直驻留**
- 单次查询可用 `get_llm_secret()`，它先查 store、再回退环境变量
  （`apps/shared/src/llm_secrets.rs:228-241`）

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
├── types.rs                        # KeyMaterial, KeyProviderConfig
├── key_provider_error.rs           # KeyProviderError
├── whitebox.rs                     # WhiteBoxKeyProvider 实现
├── sdf.rs                          # SDF 国密接口封装 + TeeKeyProvider
└── hsm.rs                          # HSM PKCS#11 接口（预留）
```

### 4.2 apps/shared (Secrets 存储)

Secrets 存储与解密逻辑在 `xiaoo-shared` crate 中，CLI/TUI/Daemon 共用。

```
apps/shared/src/
└── llm_secrets.rs                  # 本地加密存储管理
                                    # - save_llm_secret() / delete_llm_secret()
                                    # - get_llm_secret()
                                    # - inject_llm_secrets_into_env()
                                    # - inspect_secret_store()
                                    # - encrypt_aes_gcm() / decrypt_aes_gcm()
                                    # - auto_save_from_env()（已定义但无调用方）
                                    # - llm_secrets_path()
```

> 早期版本的文档在此处描述了一个 `gateway/decrypted_api_keys.rs` 模块，导出
> `SecretProvider`、`init_secret_provider()`、`get_decrypted_api_key()`。**该模块与这些
> 符号均不存在**（`git grep -n 'decrypted_api_keys\|SecretProvider\|init_secret_provider\|get_decrypted_api_key' -- '*.rs'`
> 无匹配）。真实的公开 API 就是上表列出的四个函数
> （`apps/shared/src/llm_secrets.rs:228-266`）。

### 4.3 apps/endside (TUI 配置入口)

```
apps/endside/src/
├── main.rs                         # TUI 启动入口，调用 inject_llm_secrets_into_env()
└── support/
    └── config.rs                   # TUI 配置集成，重新导出 save_llm_secret /
                                    # inject_llm_secrets_into_env
```

`apps/endside/src/main.rs:76` 调用 `config::inject_llm_secrets_into_env()`；
`apps/endside/src/support/config.rs:457-462` 是这两个函数的薄转发。

### 4.4 各文件作用

| 文件 | 作用 |
|------|------|
| `apps/vault/src/whitebox.rs` | 白盒密钥，从代码碎片重建 master key |
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
│  │              │  vault (whitebox/sdf) │                │    │
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
| `use_sdf` | bool | `false` | false=WhiteBox(仅测试)，true=SDF国密(仅鲲鹏服务器) |

#### 5.2.2 配置文件示例

**vault.enabled = true (加密存储 - WhiteBox)**

```toml
[vault]
enabled = true
use_sdf = false  # 仅测试环境使用

[llm]
provider = "openrouter"
model = "anthropic/claude-sonnet-4"   # required when [llm.profiles] is absent
api_key_env = "OPENROUTER_API_KEY"
```

> ⚠️ **WhiteBox 警告**: `use_sdf=false` 仅适用于测试环境，生产环境请使用 `use_sdf=true`。

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
| 任意 `enabled` 取值 | **无影响** —— 只有显式 `config set-secret` 才写入文件 | 若 `llm_secrets.json` 存在，解密后注入进程环境变量 |
| `use_sdf=false` | `set-secret` 用 WhiteBox+AES-GCM 加密 | 用 WhiteBox 密钥解密（仅测试） |
| `use_sdf=true` | `set-secret` 用 SDF 国密加密 | 用 SDF 密钥解密（仅鲲鹏） |

真实的加密方式选择来自 `[vault] use_sdf`，由 `get_use_sdf_from_config()` 读取
（`apps/shared/src/llm_secrets.rs:22-36`）。

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
# 默认编译 (WhiteBox + AES-GCM，仅测试环境)
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

# 运行 TUI (vault.enabled=true 时自动保存)
./target/release/xiaoo --config config.toml

# 检查加密文件
hexdump -C ~/.xiaoo/config/llm_secrets.json | head
```

---

## 6. 关键 API

### 6.1 llm_secrets 模块（真实公开 API）

以下为 `apps/shared/src/llm_secrets.rs` 中实际存在的公开函数：

```rust
// 写入 / 删除单个密钥（按 environment 变量名索引）
pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()>;   // :40-56
pub fn delete_llm_secret(config_path: &Path, env_name: &str) -> Result<bool>;            // :58-77

// 解析单个密钥（先查 store，再回退进程环境变量）
pub fn get_llm_secret(config_path: &Path, env_name: &str) -> Result<String>;             // :228-241

// 解密整个 store 并注入进程环境变量（启动路径使用）
pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()>;                    // :245-254

// 只返回密钥名称，不暴露值
pub fn inspect_secret_store(config_path: &Path) -> Result<SecretStoreMetadata>;          // :257-266

// 存储路径
pub fn llm_secrets_path(config_path: &Path) -> PathBuf;                                  // :219-224
```

> **不存在**：`load_llm_secrets_to_memory()` 与 `save_token()` 从未在本仓库中定义
> （`git grep -n 'load_llm_secrets_to_memory\|save_token' -- '*.rs'` 无匹配）。
> `auto_save_from_env()`（`:79`）虽然存在，但**零调用方**（§2.2 已知缺口）。

### 6.2 CLI / 参考面（此前文档缺失）

Secrets 的唯一入口是 daemon 的 `config` 子命令。全部经 `xiaoo-daemon` 提供：

| 命令 | 行为 | 位置 |
|------|------|------|
| `xiaoo-daemon config vault [--config <path>]` | 打印 `VaultReport` JSON：provider、是否可用、以及每个 `*_env` 配置项的解析状态 | `apps/serverside/src/main.rs:420-427` |
| `xiaoo-daemon config set-secret --environment <name> [--config <path>] < secret` | 从 **stdin** 读取密钥（去除尾部换行），校验该 environment 被配置引用后加密写入 | `apps/serverside/src/main.rs:428-455` |
| `xiaoo-daemon config delete-secret --environment <name> [--config <path>]` | 删除条目，输出 `{"schema_version":1,"removed":<bool>}` | `apps/serverside/src/main.rs:456-470` |

命令的用法文本见 `apps/serverside/src/main.rs:1333-1335`。

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
// WhiteBox
pub struct WhiteBoxKeyProvider { ... }
impl KeyProvider for WhiteBoxKeyProvider { ... }

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

### 7.1 WhiteBox

- 密钥在软件中重建，理论上可被内存抓取攻击获取
- 仅适用于开发/测试环境

### 7.2 SDF 国密

- 需要 libsdf.so 及依赖库
- KEK 口令默认为 NULL，需要配置真实 KEK
- libcrypto 版本需与 libsdf.so 匹配

### 7.4 实现缺口（截至 master `a58d79f`）

- **`vault.enabled` 是死配置**：运行路径从不读取，保存/注入无条件执行（§2.3）。
- **`auto_save_from_env()` 零调用方**：`apps/shared/src/llm_secrets.rs:79`，因此「首次启动
  自动从环境变量保存」不会发生；写入只能由 `config set-secret` 触发。
- **密钥常驻进程环境**：`inject_llm_secrets_into_env()` 用 `std::env::set_var` 注入
  （`apps/shared/src/llm_secrets.rs:245-254`），与 §1.2 宣称的「用完即销毁」不符。
- **无 `vault://` / `ref:` 引用语法**：配置中只能引用环境变量名（§6.2）。

---

## 附录：加密格式

### WhiteBox + AES-GCM

```
┌─────────────────────────────────────────────────────────────┐
│  Version (1 byte)  │  Nonce (12 bytes)  │  Ciphertext     │
│         1          │     random          │  (含 auth tag)   │
└─────────────────────────────────────────────────────────────┘
```

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
