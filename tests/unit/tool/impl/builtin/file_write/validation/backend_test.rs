use super::*;

#[test]
fn allows_documentation_that_mentions_secret_like_terms() {
    let input = FileWriteInput {
        file_path: "//tmp/config.txt".to_string(),
        content: "Explain how an api_key token or secret is configured.".to_string(),
    };

    let result = validate_input_with_base_from_bytes(&input);

    assert!(result.result);
    assert_eq!(result.error_code, None);
}
