//! SDF 国密接口 + TEE 密钥提供者
//!
//! 包含:
//! - SDF 国密加密/解密接口 (libsdf.so)
//! - TEE 密钥提供者 (支持 Intel SGX, ARM TrustZone, AWS Nitro, SDF 等)

use crate::key_provider::{KeyMaterial, KeyProvider, KeyProviderError};
use async_trait::async_trait;
use libloading::{Library, Symbol};

// ============================================================================
// TeeKeyProvider - TEE 密钥提供者
// ============================================================================

#[derive(Clone, Debug)]
pub enum TeeType {
    IntelSgx,
    ArmTrustZone,
    Hypervisor,
    AwsNitro,
    IntelTdx,
    Sdf,
    Custom(String),
}

impl TeeType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "intel_sgx" | "sgx" => Self::IntelSgx,
            "arm_trustzone" | "trustzone" | "tz" => Self::ArmTrustZone,
            "hypervisor" | "hvm" => Self::Hypervisor,
            "aws_nitro" | "nitro" => Self::AwsNitro,
            "intel_tdx" | "tdx" => Self::IntelTdx,
            "sdf" | "national_security_module" => Self::Sdf,
            other => Self::Custom(other.to_string()),
        }
    }
}

/// TEE 密钥提供者
pub struct TeeKeyProvider {
    name: String,
    tee_type: TeeType,
    slot: Option<String>,
}

impl TeeKeyProvider {
    pub fn new(name: impl Into<String>, tee_type: TeeType) -> Self {
        Self {
            name: name.into(),
            tee_type,
            slot: None,
        }
    }

    pub fn with_slot(mut self, slot: impl Into<String>) -> Self {
        self.slot = Some(slot.into());
        self
    }

    pub fn intel_sgx(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::IntelSgx)
    }

    pub fn arm_trustzone(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::ArmTrustZone)
    }

    pub fn aws_nitro(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::AwsNitro)
    }

    pub fn sdf(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::Sdf)
    }
}

#[async_trait]
impl KeyProvider for TeeKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        match self.tee_type {
            TeeType::IntelSgx => self.get_from_sgx().await,
            TeeType::ArmTrustZone => self.get_from_trustzone().await,
            TeeType::AwsNitro => self.get_from_nitro().await,
            TeeType::Hypervisor => self.get_from_hypervisor().await,
            TeeType::IntelTdx => self.get_from_tdx().await,
            TeeType::Sdf => self.get_from_sdf().await,
            TeeType::Custom(ref t) => Err(KeyProviderError::Tee(
                format!("unsupported TEE type: {}", t)
            )),
        }
    }

    fn provider_type(&self) -> &'static str {
        "tee"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl TeeKeyProvider {
    async fn get_from_sgx(&self) -> Result<KeyMaterial, KeyProviderError> {
        #[cfg(feature = "tee_sgx")]
        {
            return self.sgx_provision().await;
        }

        Err(KeyProviderError::ProviderNotAvailable(
            "Intel SGX not available. Compile with feature 'tee_sgx' to enable.".into()
        ))
    }

    async fn get_from_trustzone(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "ARM TrustZone not yet implemented".into()
        ))
    }

    async fn get_from_nitro(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "AWS Nitro Enclave not yet implemented".into()
        ))
    }

    async fn get_from_hypervisor(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "Hypervisor secure world not yet implemented".into()
        ))
    }

    async fn get_from_tdx(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "Intel TDX not yet implemented".into()
        ))
    }

    #[cfg(feature = "tee_sdf")]
    async fn get_from_sdf(&self) -> Result<KeyMaterial, KeyProviderError> {
        let provider = SdfKeyProvider::new(self.name(), "/usr/local/sdf/lib/libsdf.so");
        provider.provide().await
    }

    #[cfg(not(feature = "tee_sdf"))]
    async fn get_from_sdf(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "SDF (National Security Module) not available. Compile with feature 'tee_sdf' to enable.".into()
        ))
    }

    #[cfg(feature = "tee_sgx")]
    async fn sgx_provision(&self) -> Result<KeyMaterial, KeyProviderError> {
        use std::sync::OnceLock;

        static ENCLAVE_KEY: OnceLock<[u8; 32]> = OnceLock::new();

        let key = ENCLAVE_KEY.get_or_try_init(|| {
            Err(KeyProviderError::Tee("SGX key not yet initialized".into()))
        })?;

        Ok(KeyMaterial::new(*key, format!("tee_sgx_{}", self.name())))
    }
}

// ============================================================================
// SdfKeyProvider - SDF 国密密钥提供者
// ============================================================================

/// SDF 国密密钥提供者
pub struct SdfKeyProvider {
    name: String,
    library_path: String,
}

impl SdfKeyProvider {
    pub fn new(name: impl Into<String>, library_path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            library_path: library_path.into(),
        }
    }
}

#[cfg(feature = "tee_sdf")]
#[async_trait]
impl KeyProvider for SdfKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        use std::sync::OnceLock;
        use std::sync::atomic::{AtomicBool, Ordering};
        use rand::RngCore;

        static INIT: AtomicBool = AtomicBool::new(false);
        static mut KEY: [u8; 32] = [0; 32];

        if !INIT.load(Ordering::SeqCst) {
            unsafe {
                rand::thread_rng().fill_bytes(&mut KEY);
            }
            INIT.store(true, Ordering::SeqCst);
        }

        Ok(unsafe { KeyMaterial::new(KEY, format!("sdf_{}", self.name())) })
    }

    fn provider_type(&self) -> &'static str {
        "sdf"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(not(feature = "tee_sdf"))]
#[async_trait]
impl KeyProvider for SdfKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "SDF 国密接口未启用，请使用 --features tee_sdf 编译".into()
        ))
    }

    fn provider_type(&self) -> &'static str {
        "sdf"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 初始化 SDF 国密提供者
pub fn init_sdf_provider(path: &str) -> Result<(), KeyProviderError> {
    sdf_impl::load_library(path)
}

/// 使用 SDF 加密数据 (API key / token)
pub fn encrypt_secret(data: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
    sdf_impl::sdf_encrypt(data)
}

/// 使用 SDF 解密数据 (API key / token)
pub fn decrypt_secret(encrypted: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
    sdf_impl::sdf_decrypt(encrypted)
}

// ============================================================================
// SDF 国密加密接口实现
// ============================================================================

pub mod sdf_impl {
    use super::*;
    use std::sync::OnceLock;
    use std::os::raw::{c_char, c_int, c_void};

    // SDF 函数签名声明
    type SDF_OpenDevice_t = unsafe extern "C" fn(phDeviceHandle: *mut *mut c_void) -> c_int;
    type SDF_OpenSession_t = unsafe extern "C" fn(hDeviceHandle: *mut c_void, phSessionHandle: *mut *mut c_void) -> c_int;
    type SDF_CloseSession_t = unsafe extern "C" fn(hSessionHandle: *mut c_void) -> c_int;
    type SDF_CloseDevice_t = unsafe extern "C" fn(hDeviceHandle: *mut c_void) -> c_int;
    type SDF_GetKEKAccessRight_t = unsafe extern "C" fn(hSessionHandle: *mut c_void, uiKeyIndex: u32, pucPassword: *mut u8, uiPwdLength: u32) -> c_int;
    type SDF_ReleaseKEKAccessRight_t = unsafe extern "C" fn(hSessionHandle: *mut c_void, uiKeyIndex: u32) -> c_int;
    type SDF_GenerateKeyWithKEK_t = unsafe extern "C" fn(hSessionHandle: *mut c_void, uiKeyBits: u32, uiAlgID: u32, uiKEKIndex: u32, pucKey: *mut u8, puiKeyLength: *mut u32, phKeyHandle: *mut *mut c_void) -> c_int;
    type SDF_ImportKeyWithKEK_t = unsafe extern "C" fn(hSessionHandle: *mut c_void, uiAlgID: u32, uiKEKIndex: u32, pucKey: *mut u8, puiKeyLength: u32, phKeyHandle: *mut *mut c_void) -> c_int;
    type SDF_Encrypt_t = unsafe extern "C" fn(hSessionHandle: *mut c_void, hKeyHandle: *mut c_void, uiAlgID: u32, pucIV: *mut u8, pucData: *mut u8, uiDataLength: u32, pucEncData: *mut u8, puiEncDataLength: *mut u32) -> c_int;
    type SDF_Decrypt_t = unsafe extern "C" fn(hSessionHandle: *mut c_void, hKeyHandle: *mut c_void, uiAlgID: u32, pucIV: *mut u8, pucEncData: *mut u8, uiEncDataLength: u32, pucData: *mut u8, puiDataLength: *mut u32) -> c_int;

    // SDF 常量
    const SDF_ALG_SMS4_ECB: u32 = 0x401;      // SMS4 ECB 算法
    const SDF_KEY_INDEX_KEK: u32 = 0x1;
    const SDF_KEK_INDEX: u32 = 1;
    const SDF_KEY_BITS_128: u32 = 128;

    // 加密数据格式常量
    const ENCRYPTED_VERSION_SIZE: usize = 1;     // version 字段大小
    const ENCRYPTED_KEY_LENGTH_SIZE: usize = 4;  // key_length 字段大小
    const ENCRYPTED_HEADER_SIZE: usize = ENCRYPTED_VERSION_SIZE + ENCRYPTED_KEY_LENGTH_SIZE; // 最小头大小 = 5

    // 明文格式常量
    const PLAINTEXT_LENGTH_PREFIX_SIZE: usize = 4;  // u32 长度前缀
    const SMS4_BLOCK_SIZE: usize = 16;            // SMS4 ECB 块大小
    const SDF_CIPHER_TEXT_MAX_PADDING: usize = 32; // 解密输出缓冲区额外空间（PKCS7填充最大32字节）

    // 默认密钥口令 (设为 NULL 请务必采取以下安全措施)
    // 1. 通过安全的配置管理机制注入实际口令
    // 2. 确保配置文件权限限制为仅管理员可读
    // 3. 在使用完毕后从内存中清除敏感数据
    // 4. 更新相关安全文档并记录密钥管理策略
    // 注意: NULL 在 Rust 中用空切片表示,长度为0时传递给 C 即为 NULL 指针
    const DEFAULT_KEK_PASSWORD: &[u8] = &[];

    // 库句柄
    static LIBRARY: OnceLock<Library> = OnceLock::new();

    /// 加载 SDF 动态库
    pub fn load_library(path: &str) -> Result<(), KeyProviderError> {
        unsafe {
            if LIBRARY.get().is_some() {
                eprintln!("[SDF DEBUG] Library already loaded, skipping");
                return Ok(());
            }
            eprintln!("[SDF DEBUG] Loading library: {}", path);
            LIBRARY.set(Library::new(path).map_err(|e| {
                KeyProviderError::Tee(format!(
                    "加载libsdf.so失败: {}。\n\
                    提示：SDF国密需要运行在鲲鹏服务器上。\n\
                    请检查：\n\
                    1. 当前环境是否是鲲鹏(Kunpeng)服务器\n\
                    2. 是否具备TEE license\n\
                    如果不具备条件，请将config.toml中[vault]段的use_sdf设置为false",
                    e
                ))
            })?)
                .map_err(|_| KeyProviderError::Tee("库已加载".into()))?;
            eprintln!("[SDF DEBUG] Library loaded successfully");
            Ok(())
        }
    }

    /// 使用 SDF 加密数据 (API key / token)
    pub fn sdf_encrypt(data: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
        unsafe {
            let lib = LIBRARY.get().ok_or_else(|| KeyProviderError::ProviderNotAvailable("SDF库未加载".into()))?;

            let mut device_handle: *mut c_void = std::ptr::null_mut();
            let mut session_handle: *mut c_void = std::ptr::null_mut();

            // 1. 打开设备
            let open_device: Symbol<SDF_OpenDevice_t> = lib.get(b"SDF_OpenDevice")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenDevice未找到: {}", e)))?;
            let ret = (open_device)(&mut device_handle);
            if ret != 0 {
                return Err(KeyProviderError::Tee(format!("SDF_OpenDevice失败: {}", ret)));
            }

            // 2. 创建会话
            let open_session: Symbol<SDF_OpenSession_t> = lib.get(b"SDF_OpenSession")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenSession未找到: {}", e)))?;
            let ret = (open_session)(device_handle, &mut session_handle);
            if ret != 0 {
                let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!("SDF_OpenSession失败: {}", ret)));
            }

            // 3. 获取密钥使用权限
            let get_kek_right: Symbol<SDF_GetKEKAccessRight_t> = lib.get(b"SDF_GetKEKAccessRight")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_GetKEKAccessRight未找到: {}", e)))?;
            let mut password = DEFAULT_KEK_PASSWORD.to_vec();
            let ret = (get_kek_right)(session_handle, SDF_KEY_INDEX_KEK, password.as_mut_ptr(), DEFAULT_KEK_PASSWORD.len() as u32);
            if ret != 0 {
                let close_session: Symbol<SDF_CloseSession_t> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!("SDF_GetKEKAccessRight失败: {}", ret)));
            }

            // 4. 生成会话密钥密文
            let generate_key_with_kek: Symbol<SDF_GenerateKeyWithKEK_t> = lib.get(b"SDF_GenerateKeyWithKEK")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_GenerateKeyWithKEK未找到: {}", e)))?;
            let mut key_buffer = vec![0u8; 256];
            let mut key_length: u32 = 256;
            let mut key_handle: *mut c_void = std::ptr::null_mut();
            let ret = (generate_key_with_kek)(session_handle, SDF_KEY_BITS_128, SDF_ALG_SMS4_ECB, SDF_KEK_INDEX, key_buffer.as_mut_ptr(), &mut key_length, &mut key_handle);
            if ret != 0 {
                let release_kek: Symbol<SDF_ReleaseKEKAccessRight_t> = lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
                let close_session: Symbol<SDF_CloseSession_t> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
                (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!("SDF_GenerateKeyWithKEK失败: {}", ret)));
            }

            // 5. 加密数据
            let json_len = data.len() as u32;
            let len_bytes = json_len.to_be_bytes();
            let mut plaintext = Vec::with_capacity(PLAINTEXT_LENGTH_PREFIX_SIZE + data.len());
            plaintext.extend_from_slice(&len_bytes);
            plaintext.extend_from_slice(data);

            let pad_len = SMS4_BLOCK_SIZE - (plaintext.len() % SMS4_BLOCK_SIZE);
            plaintext.resize(plaintext.len() + pad_len, pad_len as u8);

            let mut ciphertext = vec![0u8; plaintext.len() + 32];
            let mut ciphertext_len: u32 = ciphertext.len() as u32;

            let encrypt: Symbol<SDF_Encrypt_t> = lib.get(b"SDF_Encrypt")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_Encrypt未找到: {}", e)))?;
            let ret = (encrypt)(
                session_handle,
                key_handle,
                SDF_ALG_SMS4_ECB,
                std::ptr::null_mut(),
                plaintext.as_mut_ptr(),
                plaintext.len() as u32,
                ciphertext.as_mut_ptr(),
                &mut ciphertext_len,
            );

            // 6. 释放资源
            let release_kek: Symbol<SDF_ReleaseKEKAccessRight_t> = lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
            let close_session: Symbol<SDF_CloseSession_t> = lib.get(b"SDF_CloseSession").unwrap();
            let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
            (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
            (close_session)(session_handle);
            (close_device)(device_handle);

            if ret != 0 {
                return Err(KeyProviderError::Tee(format!("SDF_Encrypt失败: {}", ret)));
            }

            ciphertext.truncate(ciphertext_len as usize);

            // 格式: version(1) + key_length(4) + key_buffer + ciphertext
            let mut result = vec![1u8];
            let key_len_bytes = (key_length as u32).to_be_bytes();
            result.extend(&key_len_bytes);
            result.extend(&key_buffer[..key_length as usize]);
            result.extend(ciphertext);
            Ok(result)
        }
    }

    /// 使用 SDF 解密数据 (API key / token)
    pub fn sdf_decrypt(encrypted: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
        if encrypted.len() < ENCRYPTED_HEADER_SIZE {
            return Err(KeyProviderError::InvalidInput("encrypted data too short".into()));
        }

        unsafe {
            let lib = LIBRARY.get().ok_or_else(|| KeyProviderError::ProviderNotAvailable("SDF库未加载".into()))?;

            let version = encrypted[0];
            if version != 1 {
                return Err(KeyProviderError::InvalidInput(format!("unknown version: {}", version)));
            }

            // 解析 version(1) + key_length(4) + key_buffer + ciphertext
            let key_length_offset = ENCRYPTED_VERSION_SIZE;
            let key_length = u32::from_be_bytes([
                encrypted[key_length_offset],
                encrypted[key_length_offset + 1],
                encrypted[key_length_offset + 2],
                encrypted[key_length_offset + 3],
            ]) as usize;

            let required_len = ENCRYPTED_HEADER_SIZE + key_length;
            if encrypted.len() < required_len {
                return Err(KeyProviderError::InvalidInput(format!(
                    "encrypted data too short: need {} bytes for key + ciphertext, got {}",
                    required_len,
                    encrypted.len()
                )));
            }

            let key_buffer_start = ENCRYPTED_HEADER_SIZE;
            let key_buffer_end = key_buffer_start + key_length;
            let ciphertext_start = key_buffer_end;

            let key_buffer = encrypted[key_buffer_start..key_buffer_end].to_vec();
            let ciphertext = &encrypted[ciphertext_start..];

            let mut device_handle: *mut c_void = std::ptr::null_mut();
            let mut session_handle: *mut c_void = std::ptr::null_mut();

            // 1. 打开设备
            let open_device: Symbol<SDF_OpenDevice_t> = lib.get(b"SDF_OpenDevice")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenDevice未找到: {}", e)))?;
            let ret = (open_device)(&mut device_handle);
            if ret != 0 {
                return Err(KeyProviderError::Tee(format!("SDF_OpenDevice失败: {}", ret)));
            }

            // 2. 创建会话
            let open_session: Symbol<SDF_OpenSession_t> = lib.get(b"SDF_OpenSession")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenSession未找到: {}", e)))?;
            let ret = (open_session)(device_handle, &mut session_handle);
            if ret != 0 {
                let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!("SDF_OpenSession失败: {}", ret)));
            }

            // 3. 获取密钥使用权限
            let get_kek_right: Symbol<SDF_GetKEKAccessRight_t> = lib.get(b"SDF_GetKEKAccessRight")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_GetKEKAccessRight未找到: {}", e)))?;
            let mut password = DEFAULT_KEK_PASSWORD.to_vec();
            let ret = (get_kek_right)(session_handle, SDF_KEY_INDEX_KEK, password.as_mut_ptr(), DEFAULT_KEK_PASSWORD.len() as u32);
            if ret != 0 {
                let close_session: Symbol<SDF_CloseSession_t> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!("SDF_GetKEKAccessRight失败: {}", ret)));
            }

            // 4. 导入会话密钥（从加密时存储的 key_buffer）
            let import_key: Symbol<SDF_ImportKeyWithKEK_t> = lib.get(b"SDF_ImportKeyWithKEK")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_ImportKeyWithKEK未找到: {}", e)))?;
            let mut imported_key_handle: *mut c_void = std::ptr::null_mut();
            let key_len_u32 = key_length as u32;
            let ret = (import_key)(session_handle, SDF_ALG_SMS4_ECB, SDF_KEK_INDEX, key_buffer.as_ptr() as *mut u8, key_len_u32, &mut imported_key_handle);
            if ret != 0 {
                let release_kek: Symbol<SDF_ReleaseKEKAccessRight_t> = lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
                let close_session: Symbol<SDF_CloseSession_t> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
                (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!("SDF_ImportKeyWithKEK失败: {}", ret)));
            }

            // 5. 解密数据
            let mut plaintext = vec![0u8; ciphertext.len() + SDF_CIPHER_TEXT_MAX_PADDING];
            let mut plaintext_len: u32 = plaintext.len() as u32;

            let decrypt: Symbol<SDF_Decrypt_t> = lib.get(b"SDF_Decrypt")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_Decrypt未找到: {}", e)))?;
            let ret = (decrypt)(
                session_handle,
                imported_key_handle,
                SDF_ALG_SMS4_ECB,
                std::ptr::null_mut(),
                ciphertext.as_ptr() as *mut u8,
                ciphertext.len() as u32,
                plaintext.as_mut_ptr(),
                &mut plaintext_len,
            );

            // 6. 释放资源
            let release_kek: Symbol<SDF_ReleaseKEKAccessRight_t> = lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
            let close_session: Symbol<SDF_CloseSession_t> = lib.get(b"SDF_CloseSession").unwrap();
            let close_device: Symbol<SDF_CloseDevice_t> = lib.get(b"SDF_CloseDevice").unwrap();
            (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
            (close_session)(session_handle);
            (close_device)(device_handle);

            if ret != 0 {
                return Err(KeyProviderError::Tee(format!("SDF_Decrypt失败: {}", ret)));
            }

            plaintext.truncate(plaintext_len as usize);

            // 7. 解析长度前缀
            if plaintext.len() < PLAINTEXT_LENGTH_PREFIX_SIZE {
                return Err(KeyProviderError::InvalidInput("decrypted data too short for length prefix".into()));
            }

            let json_len = u32::from_be_bytes([
                plaintext[0],
                plaintext[1],
                plaintext[2],
                plaintext[3],
            ]) as usize;

            if PLAINTEXT_LENGTH_PREFIX_SIZE + json_len > plaintext.len() {
                return Err(KeyProviderError::InvalidInput(format!(
                    "json_len {} exceeds plaintext len {}",
                    json_len,
                    plaintext.len()
                )));
            }

            let json_bytes = &plaintext[PLAINTEXT_LENGTH_PREFIX_SIZE..PLAINTEXT_LENGTH_PREFIX_SIZE + json_len];
            Ok(json_bytes.to_vec())
        }
    }

}
