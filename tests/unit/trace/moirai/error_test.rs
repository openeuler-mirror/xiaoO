use super::*;

#[test]
fn rusqlite_error_converts_to_storage_variant() {
    let db_error = rusqlite::Error::ExecuteReturnedResults;
    let converted: MoiraiError = db_error.into();

    match converted {
        MoiraiError::Storage(msg) => {
            assert!(!msg.is_empty());
        }
        _ => panic!("Expected Storage variant"),
    }
}

#[test]
fn serde_error_converts_to_serialization_variant() {
    let json_str = "{invalid json}";
    let serde_error: serde_json::Error =
        serde_json::from_str::<serde_json::Value>(json_str).unwrap_err();
    let converted: MoiraiError = serde_error.into();

    match converted {
        MoiraiError::Serialization(msg) => {
            assert!(!msg.is_empty());
        }
        _ => panic!("Expected Serialization variant"),
    }
}

#[test]
fn display_prefixes_each_variant_with_its_kind() {
    let storage_err = MoiraiError::Storage("db connection failed".to_string());
    assert_eq!(
        storage_err.to_string(),
        "Storage error: db connection failed"
    );

    let serde_err = MoiraiError::Serialization("invalid json".to_string());
    assert_eq!(serde_err.to_string(), "Serialization error: invalid json");

    let state_err = MoiraiError::InvalidState("already initialized".to_string());
    assert_eq!(state_err.to_string(), "Invalid state: already initialized");

    let not_found_err = MoiraiError::NotFound("trace not found".to_string());
    assert_eq!(not_found_err.to_string(), "Not found: trace not found");
}
