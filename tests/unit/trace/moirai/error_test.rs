use super::*;

#[test]
fn test_from_rusqlite_error() {
    let db_error = rusqlite::Error::ExecuteReturnedResults;
    let converted: MoiraiError = db_error.into();

    // Verify it's the Storage variant
    match converted {
        MoiraiError::Storage(msg) => {
            assert!(!msg.is_empty());
        }
        _ => panic!("Expected Storage variant"),
    }
}

#[test]
fn test_from_serde_error() {
    let json_str = "{invalid json}";
    let serde_error: serde_json::Error =
        serde_json::from_str::<serde_json::Value>(json_str).unwrap_err();
    let converted: MoiraiError = serde_error.into();

    // Verify it's the Serialization variant
    match converted {
        MoiraiError::Serialization(msg) => {
            assert!(!msg.is_empty());
        }
        _ => panic!("Expected Serialization variant"),
    }
}

#[test]
fn test_error_display() {
    // Test Storage variant
    let storage_err = MoiraiError::Storage("db connection failed".to_string());
    assert_eq!(
        storage_err.to_string(),
        "Storage error: db connection failed"
    );

    // Test Serialization variant
    let serde_err = MoiraiError::Serialization("invalid json".to_string());
    assert_eq!(serde_err.to_string(), "Serialization error: invalid json");

    // Test InvalidState variant
    let state_err = MoiraiError::InvalidState("already initialized".to_string());
    assert_eq!(state_err.to_string(), "Invalid state: already initialized");

    // Test NotFound variant
    let not_found_err = MoiraiError::NotFound("trace not found".to_string());
    assert_eq!(not_found_err.to_string(), "Not found: trace not found");
}
