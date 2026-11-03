use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompressionMeta {
    pub last_compact_turn: Option<u32>,
    pub compact_summary: Option<String>,
}
