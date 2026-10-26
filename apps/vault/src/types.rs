/// Key material: raw AES-256 key bytes. Not serializable or cloneable;
/// zeroed on drop.
pub struct KeyMaterial {
    /// 原始密钥字节 (32字节 for AES-256)
    pub key_bytes: [u8; 32],
}

impl Drop for KeyMaterial {
    fn drop(&mut self) {
        self.key_bytes.fill(0);
    }
}

impl KeyMaterial {
    /// 从原始字节创建
    pub fn new(key_bytes: [u8; 32]) -> Self {
        Self { key_bytes }
    }
}
