use super::*;

#[test]
fn file_read_state_is_isolated_between_sources() {
    let first = BuiltinToolSource::new(ToolRuntimeServices::default());
    let second = BuiltinToolSource::new(ToolRuntimeServices::default());

    assert!(!Arc::ptr_eq(
        &first.file_read_state,
        &second.file_read_state
    ));
}
