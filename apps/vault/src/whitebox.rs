//! Legacy v1 white-box key derivation.
//!
//! The v1 key is derived entirely from source constants and is public
//! knowledge. Kept only to decrypt legacy v1 stores so they can be migrated
//! to the per-instance master key; never use it to encrypt new data.

use crate::key_provider_error::KeyProviderError;

/// Fragment A: bytes 0-7
const FRAG_A: [u8; 8] = [0u8; 8];

/// Fragment B: bytes 8-15
const FRAG_B: [u8; 8] = [0u8; 8];

/// Fragment C: bytes 16-23
const FRAG_C: [u8; 8] = [0u8; 8];

/// Fragment D: bytes 24-31
const FRAG_D: [u8; 8] = [0u8; 8];

/// Transform matrix for the non-linear mixing step.
const TRANSFORM_MATRIX: [[u8; 4]; 4] = [[2, 5, 8, 3], [7, 9, 1, 4], [6, 2, 5, 8], [3, 7, 9, 1]];

/// Decoy data mixed into both key halves.
const DECOY_DATA: [u8; 16] = [
    0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
];

/// S-Box for the non-linear substitution step.
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

/// Derive the legacy v1 master key. The derivation is frozen: it must keep
/// producing the historically published key, otherwise legacy stores can no
/// longer be migrated.
pub fn legacy_v1_master_key() -> Result<[u8; 32], KeyProviderError> {
    let mut raw = [0u8; 32];
    for i in 0..8 {
        raw[i] = FRAG_A[i] ^ XOR_KEY_1;
        raw[i + 8] = FRAG_B[i] ^ XOR_KEY_2;
        raw[i + 16] = FRAG_C[i] ^ XOR_KEY_3;
        raw[i + 24] = FRAG_D[i] ^ XOR_KEY_4;
    }

    for chunk in raw.chunks_mut(4) {
        let block: [u8; 4] = chunk.try_into().unwrap();
        let transformed = transform_block(&block);
        chunk.copy_from_slice(&transformed);
    }

    final_diffusion(&mut raw);
    verify_key(&raw)?;

    Ok(raw)
}

fn transform_block(block: &[u8; 4]) -> [u8; 4] {
    let mut result = [0u8; 4];
    for i in 0..4 {
        for (j, &value) in block.iter().enumerate() {
            result[i] ^= galois_mul(value, TRANSFORM_MATRIX[i][j]);
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
    value.rotate_left(n.into())
}

fn final_diffusion(key: &mut [u8; 32]) {
    for i in 0..32 {
        key[i] = SBOX[key[i] as usize];
        key[i] = rotate_left(key[i], 3);
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
    if key.iter().all(|&byte| byte == 0) {
        return Err(KeyProviderError::InvalidInput(
            "whitebox: reconstructed key is NULL (all-zero)".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/vault/whitebox_test.rs"]
mod tests;
