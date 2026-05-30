//! White-box Key Provider 实现
//!
//! ⚠️ 警告: 本实现仅适用于测试环境，请勿用于生产环境
//!
//! 安全警告:
//! - 当前密钥为 NULL（全零），仅适用于测试环境
//! - 生产环境请使用 SDF 国密 (`use_sdf=true`) 或 HSM 方案
//! - 白盒密钥存在安全风险，不推荐在生产环境中使用

use crate::key_provider::{KeyMaterial, KeyProvider, KeyProviderError};
use async_trait::async_trait;

/// 密钥碎片常量
///
/// ⚠️ 警告: 当前设为全零，仅适用于测试环境
/// 生产环境请使用 SDF 国密或 HSM 方案

/// 碎片 A: 位置 0-7
const FRAG_A: [u8; 8] = [0u8; 8];

/// 碎片 B: 位置 8-15
const FRAG_B: [u8; 8] = [0u8; 8];

/// 碎片 C: 位置 16-23
const FRAG_C: [u8; 8] = [0u8; 8];

/// 碎片 D: 位置 24-31
const FRAG_D: [u8; 8] = [0u8; 8];

/// 变换矩阵 (用于非线性变换)
const TRANSFORM_MATRIX: [[u8; 4]; 4] = [
    [2, 5, 8, 3],
    [7, 9, 1, 4],
    [6, 2, 5, 8],
    [3, 7, 9, 1],
];

/// 迷惑数据 (用于混淆分析)
const DECOY_DATA: [u8; 16] = [
    0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0,
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
];

/// S-Box 表 (用于非线性替换)
const SBOX: [u8; 256] = generate_sbox();

const fn generate_sbox() -> [u8; 256] {
    let mut sbox = [0u8; 256];
    let mut i: usize = 0;
    while i < 256 {
        let mut v = i as u8;
        v = v ^ v.rotate_left(1) ^ v.rotate_left(2) ^ v.rotate_left(3) ^ v.rotate_left(4) ^ 0x63;
        sbox[i] = v;
        i += 1;
    }
    sbox
}

const XOR_KEY_1: u8 = 0x5A;
const XOR_KEY_2: u8 = 0xA5;
const XOR_KEY_3: u8 = 0x3C;
const XOR_KEY_4: u8 = 0x7E;

/// WhiteBox 密钥提供者
///
/// ⚠️ 警告: 仅适用于测试环境，生产环境请使用 SDF 国密或 HSM
pub struct WhiteBoxKeyProvider {
    name: String,
}

impl WhiteBoxKeyProvider {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
    
    pub fn with_default_name() -> Self {
        Self::new("whitebox")
    }
    
    pub fn get_key(&self) -> Result<[u8; 32], KeyProviderError> {
        self.reconstruct_key()
    }
}

#[async_trait]
impl KeyProvider for WhiteBoxKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        let key = self.reconstruct_key()?;
        Ok(KeyMaterial::new(key, "whitebox_derived".to_string()))
    }
    
    fn provider_type(&self) -> &'static str {
        "whitebox"
    }
    
    fn name(&self) -> &str {
        &self.name
    }
}

impl WhiteBoxKeyProvider {
    fn reconstruct_key(&self) -> Result<[u8; 32], KeyProviderError> {
        let mut raw = [0u8; 32];
        for i in 0..8 {
            raw[i] = FRAG_A[i] ^ XOR_KEY_1;
            raw[i + 8] = FRAG_B[i] ^ XOR_KEY_2;
            raw[i + 16] = FRAG_C[i] ^ XOR_KEY_3;
            raw[i + 24] = FRAG_D[i] ^ XOR_KEY_4;
        }
        
        for chunk in raw.chunks_mut(4) {
            let block: [u8; 4] = chunk.try_into().unwrap();
            let transformed = Self::transform_block(&block);
            chunk.copy_from_slice(&transformed);
        }
        
        Self::final_diffusion(&mut raw);
        Self::verify_key(&raw)?;
        
        Ok(raw)
    }
    
    fn transform_block(block: &[u8; 4]) -> [u8; 4] {
        let mut result = [0u8; 4];
        for i in 0..4 {
            for j in 0..4 {
                result[i] ^= Self::galois_mul(block[j], TRANSFORM_MATRIX[i][j]);
            }
        }
        result
    }
    
    #[inline(always)]
    fn galois_mul(a: u8, b: u8) -> u8 {
        let mut p: u8 = 0;
        let mut a = a;
        let mut b = b;
        for _ in 0..8 {
            if b & 1 != 0 {
                p ^= a;
            }
            let hi_bit = a & 0x80;
            a <<= 1;
            if hi_bit != 0 {
                a ^= 0x1B;
            }
            b >>= 1;
        }
        p
    }
    
    #[inline(always)]
    fn rotate_left(value: u8, n: u8) -> u8 {
        (value << n) | (value >> (8 - n))
    }
    
    fn final_diffusion(key: &mut [u8; 32]) {
        for i in 0..32 {
            key[i] = SBOX[key[i] as usize];
            key[i] = Self::rotate_left(key[i], 3);
            if i > 0 {
                key[i] ^= key[i - 1];
            }
        }
        
        for i in 0..16 {
            key[i] ^= DECOY_DATA[i];
            key[i + 16] ^= DECOY_DATA[i];
        }
    }
    
    fn verify_key(key: &[u8; 32]) -> Result<(), KeyProviderError> {
        // ⚠️ 警告: 当前实现允许 NULL 密钥 (全零)
        // 仅适用于测试环境
        // 生产环境应验证密钥有效性
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_key_reconstruction_deterministic() {
        let provider = WhiteBoxKeyProvider::new("test");
        let key1 = provider.reconstruct_key().unwrap();
        let key2 = provider.reconstruct_key().unwrap();
        assert_eq!(key1, key2, "key reconstruction should be deterministic");
    }
    
    #[test]
    fn test_key_providers_same_key() {
        let p1 = WhiteBoxKeyProvider::new("p1");
        let p2 = WhiteBoxKeyProvider::new("p2");
        let k1 = p1.reconstruct_key().unwrap();
        let k2 = p2.reconstruct_key().unwrap();
        assert_eq!(k1, k2, "same fragments should produce same key");
    }
    
    #[test]
    fn test_key_length() {
        let provider = WhiteBoxKeyProvider::new("test");
        let key = provider.reconstruct_key().unwrap();
        assert_eq!(key.len(), 32, "AES-256 requires 32 bytes");
    }
}