/// Lossy UTF-8 decoding with incomplete code points retained between chunks.
#[derive(Default)]
pub(super) struct StreamUtf8Decoder {
    pending: Vec<u8>,
}

impl StreamUtf8Decoder {
    pub(super) fn push(&mut self, bytes: &[u8], output: &mut String) {
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0;
        while consumed < self.pending.len() {
            let remaining = &self.pending[consumed..];
            match std::str::from_utf8(remaining) {
                Ok(text) => {
                    output.push_str(text);
                    consumed = self.pending.len();
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    output.push_str(std::str::from_utf8(&remaining[..valid]).unwrap());
                    consumed += valid;
                    match error.error_len() {
                        Some(invalid) => {
                            output.push('\u{fffd}');
                            consumed += invalid;
                        }
                        None => break,
                    }
                }
            }
        }
        self.pending.drain(..consumed);
    }

    /// At EOF an incomplete code point uses the same replacement as from_utf8_lossy.
    pub(super) fn finish(&mut self, output: &mut String) {
        output.push_str(&String::from_utf8_lossy(&self.pending));
        self.pending.clear();
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/providers/stream_utf8_test.rs"]
mod tests;
