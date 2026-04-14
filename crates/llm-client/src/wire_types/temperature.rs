use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Serializes with exactly 2 decimal places to avoid floating-point artifacts
/// (e.g., 0.7 instead of 0.6999999999999999) that some providers like Zhipu reject.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub(crate) struct Temperature(f32);

impl Temperature {
    pub(crate) fn new(value: f32) -> Self {
        Self(value)
    }
}

impl fmt::Debug for Temperature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Temperature({})", self.0)
    }
}

impl fmt::Display for Temperature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Temperature> for f32 {
    fn from(temp: Temperature) -> f32 {
        temp.0
    }
}

impl From<f32> for Temperature {
    fn from(value: f32) -> Self {
        Self(value)
    }
}

impl Serialize for Temperature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let rounded = (self.0 as f64 * 100.0).round() / 100.0;
        serializer.serialize_f64(rounded)
    }
}

impl<'de> Deserialize<'de> for Temperature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f32::deserialize(deserializer)?;
        Ok(Self(value))
    }
}
