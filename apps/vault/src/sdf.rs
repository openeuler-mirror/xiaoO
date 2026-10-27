//! SDF national-crypto interface + TEE key providers.
//!
//! Contains:
//! - SDF encrypt/decrypt interface (libsdf.so)
//! - TEE key providers (SDF is the only wired backend; the rest are
//!   placeholders)

use crate::key_provider::{KeyMaterial, KeyProvider, KeyProviderError};
use async_trait::async_trait;
use libloading::{Library, Symbol};

// ============================================================================
// TeeKeyProvider — TEE key provider
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
    pub fn from_name(s: &str) -> Self {
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

impl From<&str> for TeeType {
    fn from(value: &str) -> Self {
        Self::from_name(value)
    }
}

/// TEE key provider.
///
/// Only the SDF type is usable (delegates to [`SdfKeyProvider`]); the other
/// TEE backends are unwired and their `provide()` always returns
/// `ProviderNotAvailable`.
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
            TeeType::Custom(ref t) => Err(KeyProviderError::Tee(format!(
                "unsupported TEE type: {}",
                t
            ))),
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
        Err(KeyProviderError::ProviderNotAvailable(
            "Intel SGX provider is not linked in this build".into(),
        ))
    }

    async fn get_from_trustzone(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "ARM TrustZone not yet implemented".into(),
        ))
    }

    async fn get_from_nitro(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "AWS Nitro Enclave not yet implemented".into(),
        ))
    }

    async fn get_from_hypervisor(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "Hypervisor secure world not yet implemented".into(),
        ))
    }

    async fn get_from_tdx(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "Intel TDX not yet implemented".into(),
        ))
    }

    #[cfg(feature = "tee_sdf")]
    async fn get_from_sdf(&self) -> Result<KeyMaterial, KeyProviderError> {
        let provider = SdfKeyProvider::new(self.name());
        provider.provide().await
    }

    #[cfg(not(feature = "tee_sdf"))]
    async fn get_from_sdf(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "SDF (National Security Module) not available. Compile with feature 'tee_sdf' to enable.".into()
        ))
    }
}

// ============================================================================
// SdfKeyProvider — SDF key provider
// ============================================================================

/// SDF key provider.
///
/// Note: the key material itself does not come from SDF hardware —
/// `provide()` generates a software-random key in-process and caches it
/// (OnceLock). The SDF device only takes part in the data encrypt/decrypt
/// path of [`encrypt_secret`] / [`decrypt_secret`].
pub struct SdfKeyProvider {
    name: String,
}

impl SdfKeyProvider {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[cfg(feature = "tee_sdf")]
#[async_trait]
impl KeyProvider for SdfKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        use rand::RngCore;
        use std::sync::OnceLock;

        static KEY: OnceLock<[u8; 32]> = OnceLock::new();

        let key = KEY.get_or_init(|| {
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            key
        });

        Ok(KeyMaterial::new(*key))
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
            "SDF 国密接口未启用，请使用 --features tee_sdf 编译".into(),
        ))
    }

    fn provider_type(&self) -> &'static str {
        "sdf"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Default path of the SDF provider library. Callers that need a different
/// location can still pass their own path to [`init_sdf_provider`].
pub const DEFAULT_SDF_LIBRARY_PATH: &str = "/usr/local/sdf/lib/libsdf.so";

pub fn init_sdf_provider(path: &str) -> Result<(), KeyProviderError> {
    sdf_impl::load_library(path)
}

/// Encrypt short secrets such as API keys / tokens; the envelope format is
/// documented at `sdf_impl::sdf_encrypt` (version + key_length +
/// key_buffer + ciphertext).
pub fn encrypt_secret(data: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
    sdf_impl::sdf_encrypt(data)
}

pub fn decrypt_secret(encrypted: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
    sdf_impl::sdf_decrypt(encrypted)
}

// ============================================================================
// SDF encrypt/decrypt implementation
// ============================================================================

pub mod sdf_impl {
    use super::*;
    use std::os::raw::{c_int, c_void};
    use std::sync::OnceLock;

    // SDF function signature declarations
    type SdfOpenDevice = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
    type SdfOpenSession = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> c_int;
    type SdfCloseSession = unsafe extern "C" fn(*mut c_void) -> c_int;
    type SdfCloseDevice = unsafe extern "C" fn(*mut c_void) -> c_int;
    type SdfGetKekAccessRight = unsafe extern "C" fn(*mut c_void, u32, *mut u8, u32) -> c_int;
    type SdfReleaseKekAccessRight = unsafe extern "C" fn(*mut c_void, u32) -> c_int;
    type SdfGenerateKeyWithKek = unsafe extern "C" fn(
        *mut c_void,
        u32,
        u32,
        u32,
        *mut u8,
        *mut u32,
        *mut *mut c_void,
    ) -> c_int;
    type SdfImportKeyWithKek =
        unsafe extern "C" fn(*mut c_void, u32, u32, *mut u8, u32, *mut *mut c_void) -> c_int;
    type SdfEncrypt = unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        u32,
        *mut u8,
        *mut u8,
        u32,
        *mut u8,
        *mut u32,
    ) -> c_int;
    type SdfDecrypt = unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        u32,
        *mut u8,
        *mut u8,
        u32,
        *mut u8,
        *mut u32,
    ) -> c_int;

    // SDF constants
    const SDF_ALG_SMS4_ECB: u32 = 0x401;
    const SDF_KEY_INDEX_KEK: u32 = 0x1;
    const SDF_KEK_INDEX: u32 = 1;
    const SDF_KEY_BITS_128: u32 = 128;

    // Envelope format constants
    const ENCRYPTED_VERSION_SIZE: usize = 1;
    const ENCRYPTED_KEY_LENGTH_SIZE: usize = 4;
    const ENCRYPTED_HEADER_SIZE: usize = ENCRYPTED_VERSION_SIZE + ENCRYPTED_KEY_LENGTH_SIZE;

    // Plaintext format constants
    const PLAINTEXT_LENGTH_PREFIX_SIZE: usize = 4;
    const SMS4_BLOCK_SIZE: usize = 16;
    // Decrypt output buffer headroom for PKCS7 padding (max 32 bytes).
    const SDF_CIPHER_TEXT_MAX_PADDING: usize = 32;

    // The KEK passphrase is NULL: an empty slice of length 0 becomes a NULL
    // pointer in C. Deployments that need a real passphrase must inject it
    // via secure config management, keep the config admin-only readable,
    // scrub it from memory after use, and document the key-management
    // policy.
    const DEFAULT_KEK_PASSWORD: &[u8] = &[];

    // Library handle
    static LIBRARY: OnceLock<Library> = OnceLock::new();

    /// Load the SDF dynamic library.
    pub fn load_library(path: &str) -> Result<(), KeyProviderError> {
        unsafe {
            if LIBRARY.get().is_some() {
                return Ok(());
            }
            LIBRARY
                .set(Library::new(path).map_err(|e| {
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
            Ok(())
        }
    }

    /// Encrypt data via SDF (API keys / tokens).
    pub fn sdf_encrypt(data: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
        unsafe {
            let lib = LIBRARY
                .get()
                .ok_or_else(|| KeyProviderError::ProviderNotAvailable("SDF库未加载".into()))?;

            let mut device_handle: *mut c_void = std::ptr::null_mut();
            let mut session_handle: *mut c_void = std::ptr::null_mut();

            let open_device: Symbol<SdfOpenDevice> = lib
                .get(b"SDF_OpenDevice")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenDevice未找到: {}", e)))?;
            let ret = (open_device)(&mut device_handle);
            if ret != 0 {
                return Err(KeyProviderError::Tee(format!(
                    "SDF_OpenDevice失败: {}",
                    ret
                )));
            }

            let open_session: Symbol<SdfOpenSession> = lib
                .get(b"SDF_OpenSession")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenSession未找到: {}", e)))?;
            let ret = (open_session)(device_handle, &mut session_handle);
            if ret != 0 {
                let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!(
                    "SDF_OpenSession失败: {}",
                    ret
                )));
            }

            let get_kek_right: Symbol<SdfGetKekAccessRight> =
                lib.get(b"SDF_GetKEKAccessRight").map_err(|e| {
                    KeyProviderError::Tee(format!("SDF_GetKEKAccessRight未找到: {}", e))
                })?;
            let mut password = DEFAULT_KEK_PASSWORD.to_vec();
            let ret = (get_kek_right)(
                session_handle,
                SDF_KEY_INDEX_KEK,
                password.as_mut_ptr(),
                DEFAULT_KEK_PASSWORD.len() as u32,
            );
            if ret != 0 {
                let close_session: Symbol<SdfCloseSession> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!(
                    "SDF_GetKEKAccessRight失败: {}",
                    ret
                )));
            }

            let generate_key_with_kek: Symbol<SdfGenerateKeyWithKek> =
                lib.get(b"SDF_GenerateKeyWithKEK").map_err(|e| {
                    KeyProviderError::Tee(format!("SDF_GenerateKeyWithKEK未找到: {}", e))
                })?;
            let mut key_buffer = vec![0u8; 256];
            let mut key_length: u32 = 256;
            let mut key_handle: *mut c_void = std::ptr::null_mut();
            let ret = (generate_key_with_kek)(
                session_handle,
                SDF_KEY_BITS_128,
                SDF_ALG_SMS4_ECB,
                SDF_KEK_INDEX,
                key_buffer.as_mut_ptr(),
                &mut key_length,
                &mut key_handle,
            );
            if ret != 0 {
                let release_kek: Symbol<SdfReleaseKekAccessRight> =
                    lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
                let close_session: Symbol<SdfCloseSession> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
                (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!(
                    "SDF_GenerateKeyWithKEK失败: {}",
                    ret
                )));
            }

            let json_len = data.len() as u32;
            let len_bytes = json_len.to_be_bytes();
            let mut plaintext = Vec::with_capacity(PLAINTEXT_LENGTH_PREFIX_SIZE + data.len());
            plaintext.extend_from_slice(&len_bytes);
            plaintext.extend_from_slice(data);

            let pad_len = SMS4_BLOCK_SIZE - (plaintext.len() % SMS4_BLOCK_SIZE);
            plaintext.resize(plaintext.len() + pad_len, pad_len as u8);

            let mut ciphertext = vec![0u8; plaintext.len() + 32];
            let mut ciphertext_len: u32 = ciphertext.len() as u32;

            let encrypt: Symbol<SdfEncrypt> = lib
                .get(b"SDF_Encrypt")
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

            let release_kek: Symbol<SdfReleaseKekAccessRight> =
                lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
            let close_session: Symbol<SdfCloseSession> = lib.get(b"SDF_CloseSession").unwrap();
            let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
            (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
            (close_session)(session_handle);
            (close_device)(device_handle);

            if ret != 0 {
                return Err(KeyProviderError::Tee(format!("SDF_Encrypt失败: {}", ret)));
            }

            ciphertext.truncate(ciphertext_len as usize);

            // Envelope: version(1) + key_length(4) + key_buffer + ciphertext
            let mut result = vec![1u8];
            let key_len_bytes = key_length.to_be_bytes();
            result.extend(&key_len_bytes);
            result.extend(&key_buffer[..key_length as usize]);
            result.extend(ciphertext);
            Ok(result)
        }
    }

    /// Decrypt data via SDF (API keys / tokens).
    pub fn sdf_decrypt(encrypted: &[u8]) -> Result<Vec<u8>, KeyProviderError> {
        if encrypted.len() < ENCRYPTED_HEADER_SIZE {
            return Err(KeyProviderError::InvalidInput(
                "encrypted data too short".into(),
            ));
        }

        unsafe {
            let lib = LIBRARY
                .get()
                .ok_or_else(|| KeyProviderError::ProviderNotAvailable("SDF库未加载".into()))?;

            let version = encrypted[0];
            if version != 1 {
                return Err(KeyProviderError::InvalidInput(format!(
                    "unknown version: {}",
                    version
                )));
            }

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

            let open_device: Symbol<SdfOpenDevice> = lib
                .get(b"SDF_OpenDevice")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenDevice未找到: {}", e)))?;
            let ret = (open_device)(&mut device_handle);
            if ret != 0 {
                return Err(KeyProviderError::Tee(format!(
                    "SDF_OpenDevice失败: {}",
                    ret
                )));
            }

            let open_session: Symbol<SdfOpenSession> = lib
                .get(b"SDF_OpenSession")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_OpenSession未找到: {}", e)))?;
            let ret = (open_session)(device_handle, &mut session_handle);
            if ret != 0 {
                let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!(
                    "SDF_OpenSession失败: {}",
                    ret
                )));
            }

            let get_kek_right: Symbol<SdfGetKekAccessRight> =
                lib.get(b"SDF_GetKEKAccessRight").map_err(|e| {
                    KeyProviderError::Tee(format!("SDF_GetKEKAccessRight未找到: {}", e))
                })?;
            let mut password = DEFAULT_KEK_PASSWORD.to_vec();
            let ret = (get_kek_right)(
                session_handle,
                SDF_KEY_INDEX_KEK,
                password.as_mut_ptr(),
                DEFAULT_KEK_PASSWORD.len() as u32,
            );
            if ret != 0 {
                let close_session: Symbol<SdfCloseSession> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!(
                    "SDF_GetKEKAccessRight失败: {}",
                    ret
                )));
            }

            let import_key: Symbol<SdfImportKeyWithKek> = lib
                .get(b"SDF_ImportKeyWithKEK")
                .map_err(|e| KeyProviderError::Tee(format!("SDF_ImportKeyWithKEK未找到: {}", e)))?;
            let mut imported_key_handle: *mut c_void = std::ptr::null_mut();
            let key_len_u32 = key_length as u32;
            let ret = (import_key)(
                session_handle,
                SDF_ALG_SMS4_ECB,
                SDF_KEK_INDEX,
                key_buffer.as_ptr() as *mut u8,
                key_len_u32,
                &mut imported_key_handle,
            );
            if ret != 0 {
                let release_kek: Symbol<SdfReleaseKekAccessRight> =
                    lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
                let close_session: Symbol<SdfCloseSession> = lib.get(b"SDF_CloseSession").unwrap();
                let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
                (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
                (close_session)(session_handle);
                (close_device)(device_handle);
                return Err(KeyProviderError::Tee(format!(
                    "SDF_ImportKeyWithKEK失败: {}",
                    ret
                )));
            }

            let mut plaintext = vec![0u8; ciphertext.len() + SDF_CIPHER_TEXT_MAX_PADDING];
            let mut plaintext_len: u32 = plaintext.len() as u32;

            let decrypt: Symbol<SdfDecrypt> = lib
                .get(b"SDF_Decrypt")
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

            let release_kek: Symbol<SdfReleaseKekAccessRight> =
                lib.get(b"SDF_ReleaseKEKAccessRight").unwrap();
            let close_session: Symbol<SdfCloseSession> = lib.get(b"SDF_CloseSession").unwrap();
            let close_device: Symbol<SdfCloseDevice> = lib.get(b"SDF_CloseDevice").unwrap();
            (release_kek)(session_handle, SDF_KEY_INDEX_KEK);
            (close_session)(session_handle);
            (close_device)(device_handle);

            if ret != 0 {
                return Err(KeyProviderError::Tee(format!("SDF_Decrypt失败: {}", ret)));
            }

            plaintext.truncate(plaintext_len as usize);

            if plaintext.len() < PLAINTEXT_LENGTH_PREFIX_SIZE {
                return Err(KeyProviderError::InvalidInput(
                    "decrypted data too short for length prefix".into(),
                ));
            }

            let json_len =
                u32::from_be_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]])
                    as usize;

            if PLAINTEXT_LENGTH_PREFIX_SIZE + json_len > plaintext.len() {
                return Err(KeyProviderError::InvalidInput(format!(
                    "json_len {} exceeds plaintext len {}",
                    json_len,
                    plaintext.len()
                )));
            }

            let json_bytes =
                &plaintext[PLAINTEXT_LENGTH_PREFIX_SIZE..PLAINTEXT_LENGTH_PREFIX_SIZE + json_len];
            Ok(json_bytes.to_vec())
        }
    }
}
