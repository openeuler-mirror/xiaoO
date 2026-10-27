/// Key material: raw AES-256 key bytes. Not serializable or cloneable;
/// zeroed on drop.
pub struct KeyMaterial {
    pub key_bytes: [u8; 32],
}

impl Drop for KeyMaterial {
    fn drop(&mut self) {
        self.key_bytes.fill(0);
    }
}

impl KeyMaterial {
    pub fn new(key_bytes: [u8; 32]) -> Self {
        Self { key_bytes }
    }
}
