use serde::{Deserialize, Serialize};

/// 密钥材料
/// 
/// 包含加密所需的密钥字节
#[derive(Clone, Serialize, Deserialize)]
pub struct KeyMaterial {
    /// 原始密钥字节 (32字节 for AES-256)
    pub key_bytes: [u8; 32],
    /// 密钥版本 (用于轮转)
    pub version: u32,
    /// 密钥标识符
    pub key_id: String,
    /// 创建时间戳
    pub created_at: i64,
}

impl KeyMaterial {
    /// 从原始字节创建
    pub fn new(key_bytes: [u8; 32], key_id: String) -> Self {
        Self {
            key_bytes,
            version: 1,
            key_id,
            created_at: chrono::Utc::now().timestamp(),
        }
    }
}

/// 密钥提供者配置
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum KeyProviderConfig {
    /// 白盒密钥配置
    WhiteBox {
        #[serde(default = "default_whitebox_name")]
        name: String,
    },
    /// TEE 密钥配置
    Tee {
        name: String,
        tee_type: String,
        #[serde(default)]
        slot: Option<String>,
    },
    /// HSM 密钥配置  
    Hsm {
        name: String,
        library_path: String,
        slot_id: String,
        key_label: String,
    },
}

fn default_whitebox_name() -> String {
    "whitebox".to_string()
}

impl KeyProviderConfig {
    pub fn whitebox(name: impl Into<String>) -> Self {
        Self::WhiteBox { name: name.into() }
    }
    
    pub fn tee(name: impl Into<String>, tee_type: impl Into<String>) -> Self {
        Self::Tee { 
            name: name.into(), 
            tee_type: tee_type.into(),
            slot: None,
        }
    }
    
    pub fn hsm(name: impl Into<String>, library_path: impl Into<String>, slot_id: impl Into<String>, key_label: impl Into<String>) -> Self {
        Self::Hsm { 
            name: name.into(), 
            library_path: library_path.into(),
            slot_id: slot_id.into(),
            key_label: key_label.into(),
        }
    }
}