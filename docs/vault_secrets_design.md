# Secrets 存储方案设计

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
enabled = true   # 是否启用加密存储
use_sdf = false  # false=WhiteBox+AES-GCM, true=SDF国密
```

### 2.2 vault.enabled = true 时

#### 数据保存方法

1. **首次启动**：从环境变量读取 API Key → 加密 → 保存到 `llm_secrets.json`
2. **后续启动**：从 `llm_secrets.json` 加载 → 解密 → 按需提供给调用方

#### 数据存储位置

```
{config_dir}/
└── llm_secrets.json    # 加密后的 Secrets 数据
```

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

文件内容为加密后的二进制数据。

#### 数据获取方式

**按需获取，用完即销毁**：
- 发起 LLM 请求时从加密文件解密获取 API key
- 请求完成后立即销毁，不在内存中长期驻留
- 每次请求都会重新解密（除非文件不存在才 fallback 到环境变量）

### 2.3 vault.enabled = false 时

#### 数据保存方法

不保存任何 Secrets 到文件。

#### 数据获取方式

直接使用环境变量和 config.toml 配置。

### 2.4 对比总结

| 配置 | 数据保存 | 数据获取 | 安全性 |
|------|---------|---------|--------|
| `vault.enabled=true` | 加密保存到 `llm_secrets.json` | 按需解密获取，用完即销毁 | 高 |
| `vault.enabled=false` | 不保存 | 直接使用环境变量 | 低 |

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

### 3.4 按需密钥获取机制

解密后的 Secrets 不驻留内存，采用按需获取方式：

```rust
pub fn get_decrypted_api_key(env_name: &str) -> Option<String>
pub fn init_secret_provider(secrets_path: PathBuf, use_sdf: bool)
```

**特点**：
- 每次 LLM 请求时按需解密获取 API key
- 请求完成后立即销毁，不在内存中长期驻留
- 通过 `SecretProvider` 统一管理解密逻辑

### 3.5 API Key 解析优先级

```
1. config.api_key (直接配置)
       ↓
2. llm_secrets.json (按需解密获取)
       ↓
3. 环境变量 (fallback)
```

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

### 4.2 apps/xiaoo-app

Secrets 存储和加载逻辑。

```
apps/xiaoo-app/src/
├── llm_secrets.rs                 # 本地加密存储管理
│                                 # - auto_save_from_env()
│                                 # - load_llm_secrets_to_memory()
│                                 # - encrypt_aes_gcm() / decrypt_aes_gcm()
├── secrets.rs                     # SecretsManager (Daemon 使用)
├── gateway/
│   ├── mod.rs                    # 导出
│   ├── decrypted_api_keys.rs     # SecretProvider 按需解密
│   └── ...                      # 其他 gateway 模块
└── tui/support/
    └── config.rs                 # TUI 配置集成
```

### 4.3 各文件作用

| 文件 | 作用 |
|------|------|
| `vault/src/whitebox.rs` | 白盒密钥，从代码碎片重建 master key |
| `vault/src/sdf.rs` | SDF 国密接口封装，包含 `encrypt_secret`/`decrypt_secret` |
| `vault/src/hsm.rs` | HSM PKCS#11 接口（预留） |
| `llm_secrets.rs` | 加密/解密，加密文件读写 |
| `decrypted_api_keys.rs` | `SecretProvider` 按需解密机制 |
| `config.rs` | TUI 启动时初始化 `SecretProvider` |

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
│  │  │  xiaoo-tui   │  │  xiaoo       │  │ xiaoo-app    │  │    │
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
api_key_env = "OPENROUTER_API_KEY"
```

#### 5.2.3 配置行为说明

| 配置 | 首次启动 | 后续启动 |
|------|---------|---------|
| `enabled=false` | 不保存，直接用环境变量 | 不保存，直接用环境变量 |
| `enabled=true, use_sdf=false` | 环境变量→AES-GCM加密保存(仅测试) | 从文件按需解密(仅测试) |
| `enabled=true, use_sdf=true` | 环境变量→SDF加密保存(仅鲲鹏) | 从文件按需解密(仅鲲鹏) |

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
cargo build --release --bin xiaoo-tui

# 启用 SDF 国密 (仅鲲鹏服务器)
cargo build --release --bin xiaoo-tui --features tee_sdf
```

#### 单元测试

```bash
cargo test --package vault
cargo test --package xiaoo-app
```

#### 功能验证

```bash
# 设置环境变量
export OPENROUTER_API_KEY='sk-or-v1-xxx'
export USE_SDF=false

# 运行 TUI (vault.enabled=true 时自动保存)
./target/release/xiaoo-tui --config config.toml

# 检查加密文件
hexdump -C ~/.xiaoo/config/llm_secrets.json | head
```

---

## 6. 关键 API

### 6.1 llm_secrets 模块

```rust
// 自动从环境变量保存到加密文件
pub fn auto_save_from_env(config_path: &Path) -> Result<()>;

// 加载加密文件到进程内存
pub fn load_llm_secrets_to_memory(config_path: &Path) -> Result<()>;

// 保存 API Key
pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()>;

// 保存 Token
pub fn save_token(config_path: &Path, token_name: &str, token: &str) -> Result<()>;
```

### 6.2 按需密钥获取 API

```rust
// 初始化 SecretProvider
pub fn init_secret_provider(secrets_path: PathBuf, use_sdf: bool);

// 按需获取密钥（每次解密，用完即销毁）
pub fn get_decrypted_api_key(env_name: &str) -> Option<String>;
```

### 6.3 密钥提供者

```rust
// WhiteBox
pub struct WhiteBoxKeyProvider { ... }
impl KeyProvider for WhiteBoxKeyProvider { ... }

// SDF 国密
pub fn init_sdf_provider(path: &str) -> Result<()>;
pub fn encrypt_secret(data: &[u8]) -> Result<Vec<u8>>;
pub fn decrypt_secret(encrypted: &[u8]) -> Result<Vec<u8>>;

// TEE 密钥提供者
pub struct TeeKeyProvider { ... }
impl KeyProvider for TeeKeyProvider { ... }

// HSM (预留)
pub struct HsmKeyProvider { ... }
impl KeyProvider for HsmKeyProvider { ... }
```

---

## 7. 已知限制

### 7.1 WhiteBox

- 密钥在软件中重建，理论上可被内存抓取攻击获取
- 仅适用于开发/测试环境

### 7.2 SDF 国密

- 需要 libsdf.so 及依赖库
- KEK 口令默认为 NULL，需要配置真实 KEK
- libcrypto 版本需与 libsdf.so 匹配

### 7.3 HSM

- 接口预留，实现待完成

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
┌─────────────────────────────────────────────────────────────┐
│  Version (1 byte)  │  IV (16 bytes)     │  Ciphertext     │
│         1          │     random          │  (国密加密结果)    │
└─────────────────────────────────────────────────────────────┘
```
