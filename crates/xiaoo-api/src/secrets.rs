//! LLM 密钥的加密存取。
//!
//! 【L1 支撑类型层】
//!
//! ## 调用顺序（模块文档）
//!
//! 1. 进程启动 → [`init_on_demand_secret_provider`]（或带 SDF/TEE 的
//!    [`init_secret_provider`]）；
//! 2. 运行期 [`get_decrypted_api_key`] 按需解密；
//! 3. [`auto_save_from_env`] 用于首次运行时从环境变量落盘加密。
//!
//! **初始化与按需解密已由门面内置**（[`crate::host::LocalSessionHostBuilder::secrets`]
//! + [`crate::host::LlmOptions`] 派生，阶段 2 落地）；本模块公开主要服务
//! endside 的密钥管理界面（保存 / 迁移密钥）。

// ---- 密钥文件存取（落盘加密、读取、迁移）----
#[doc(inline)]
pub use xiaoo_shared::llm_secrets::{
    auto_save_from_env, init_on_demand_secret_provider, llm_secrets_path, load_llm_secrets_to_memory,
    save_llm_secret, save_token,
};

// ---- secret provider 初始化与按需解密 ----
#[doc(inline)]
pub use xiaoo_shared::gateway::{get_decrypted_api_key, init_secret_provider};
