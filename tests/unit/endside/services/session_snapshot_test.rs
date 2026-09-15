use super::*;

fn state_with_prompt(prompt: &str) -> AppState {
    let mut state = AppState::new(PathBuf::new(), PathBuf::new()).unwrap();
    state.chat_state.messages.push(Message::user(prompt));
    state
}

fn read_snapshot(path: &Path) -> TuiSessionSnapshot {
    let key = snapshot_key_from_path(path).unwrap();
    parse_snapshot_file(path, key).unwrap()
}

fn json_file_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .count()
}

#[test]
fn test_sanitize_topic_caps_at_ten_chars() {
    // Whitespace is flattened then the first 10 chars are kept.
    assert_eq!(sanitize_topic("hello world"), "hello-worl");
    // Long ASCII prompts are truncated to 10 chars.
    assert_eq!(
        sanitize_topic("the quick brown fox jumps over the lazy dog"),
        "the-quick"
    );
}

#[test]
fn test_sanitize_topic_keeps_unicode_and_drops_punctuation() {
    // Chinese letters are alphanumeric and therefore preserved; the space
    // between words becomes a single dash.
    assert_eq!(
        sanitize_topic("帮我为xiaoo agent runtime增加会话自动保存机制"),
        "帮我为xiaoo-a"
    );
    // A prompt made solely of punctuation yields the fallback label.
    assert_eq!(sanitize_topic("!@#$%^&*()"), "untitled");
    // Whitespace-only input also falls back.
    assert_eq!(sanitize_topic("    "), "untitled");
}

#[test]
fn test_autosave_topic_uses_first_user_prompt() {
    let mut state = AppState::new(PathBuf::new(), PathBuf::new()).unwrap();
    // No user messages → nothing to summarise.
    assert_eq!(autosave_topic(&state), None);

    state
        .chat_state
        .messages
        .push(Message::user("帮我为xiaoo agent runtime增加保存机制"));
    assert_eq!(autosave_topic(&state), Some("帮我为xiaoo-a".to_string()));

    // A subsequent user prompt must not override the first one.
    state
        .chat_state
        .messages
        .push(Message::user("another unrelated question"));
    assert_eq!(autosave_topic(&state), Some("帮我为xiaoo-a".to_string()));
}

#[test]
fn manual_path_builds_parent_chain() {
    let dir = Path::new("/tmp/xiaoo-session-test");
    assert_eq!(
        manual_snapshot_path_in_dir(dir, "child", &["parent".to_string()])
            .unwrap()
            .file_name()
            .unwrap(),
        "parent_child.json"
    );
}

#[test]
fn validate_snapshot_name_reserves_auto_namespace() {
    assert!(validate_snapshot_name("test123").is_ok());
    assert!(validate_snapshot_name("test-123").is_ok());
    assert!(validate_snapshot_name("test_123").is_ok());
    assert!(validate_snapshot_name("test.123").is_ok());
    assert!(validate_snapshot_name("@auto-id").is_err());
    assert!(validate_snapshot_name("").is_err());
    assert!(validate_snapshot_name(".").is_err());
    assert!(validate_snapshot_name("..").is_err());
    assert!(validate_snapshot_name("test 123").is_err());
    assert!(validate_snapshot_name("test/123").is_err());
}

#[test]
fn autosave_from_manual_never_rewrites_manual_file() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state_with_prompt("protect this checkpoint");
    let (manual_path, manual_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("checkpoint")).unwrap();
    let manual_before = fs::read(&manual_path).unwrap();
    let manual_id = manual_context.snapshot_id.clone();
    state.current_snapshot_context = Some(manual_context);
    state
        .chat_state
        .messages
        .push(Message::system("later work"));

    let auto_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();

    assert_ne!(manual_path, auto_path);
    assert_eq!(fs::read(&manual_path).unwrap(), manual_before);
    let auto = read_snapshot(&auto_path);
    assert_eq!(auto.kind, SnapshotKind::Auto);
    assert_eq!(
        auto.base_manual
            .as_ref()
            .map(|base| base.snapshot_id.as_str()),
        Some(manual_id.as_str())
    );
}

#[test]
fn autosave_rolls_forward_one_slot_per_manual_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state_with_prompt("rolling auto save");
    let (_, first_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("first")).unwrap();
    state.current_snapshot_context = Some(first_context);
    let first_auto = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();
    let first_auto_id = read_snapshot(&first_auto).snapshot_id;

    state
        .chat_state
        .messages
        .push(Message::system("newer content"));
    let repeated_auto = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();
    assert_eq!(first_auto, repeated_auto);
    assert_eq!(read_snapshot(&repeated_auto).snapshot_id, first_auto_id);

    let (_, second_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("second")).unwrap();
    state.current_snapshot_context = Some(second_context);
    let second_auto = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();
    assert_ne!(first_auto, second_auto);

    let catalog = list_session_snapshots_in_dir(temp.path()).unwrap();
    assert_eq!(catalog.manual.len(), 2);
    assert_eq!(catalog.automatic.len(), 2);
    assert!(catalog
        .automatic
        .iter()
        .any(|entry| entry.base_manual_name.as_deref() == Some("second")));
}

#[test]
fn named_load_matches_manual_snapshots_only() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state_with_prompt("named load");
    let (_, manual_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("checkpoint")).unwrap();
    state.current_snapshot_context = Some(manual_context);
    let auto_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();
    let mut auto = read_snapshot(&auto_path);
    auto.name = "checkpoint".to_string();
    save_snapshot_at_path(&auto_path, &auto).unwrap();

    let matches = load_snapshot_in_dir(temp.path(), "checkpoint").unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].1.kind, SnapshotKind::Manual);
}

#[test]
fn loaded_auto_snapshot_overwrites_itself() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state_with_prompt("unbound work");
    let first_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();
    let first = read_snapshot(&first_path);
    state.current_snapshot_context = Some(SnapshotContext::from_snapshot(
        snapshot_key_from_path(&first_path).unwrap().to_string(),
        &first,
    ));
    state.chat_state.messages.push(Message::system("continued"));

    let second_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();

    assert_eq!(first_path, second_path);
    assert_eq!(json_file_count(temp.path()), 1);
    assert_eq!(read_snapshot(&second_path).snapshot_id, first.snapshot_id);
}

#[test]
fn explicit_same_name_overwrites_exact_nested_manual_and_preserves_id() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state_with_prompt("manual branches");
    let (_, root_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("root")).unwrap();
    state.current_snapshot_context = Some(root_context);
    let (child_path, child_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("child")).unwrap();
    let child_id = child_context.snapshot_id.clone();
    state.current_snapshot_context = Some(child_context);
    state
        .chat_state
        .messages
        .push(Message::system("updated child"));

    let (overwritten_path, overwritten_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("child")).unwrap();

    assert_eq!(child_path, overwritten_path);
    assert_eq!(overwritten_context.snapshot_id, child_id);
    assert_eq!(overwritten_context.parent_chain, vec!["root".to_string()]);
    assert!(!temp.path().join("child.json").exists());
}

#[test]
fn save_from_auto_uses_its_manual_source_as_anchor() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state_with_prompt("resume and checkpoint");
    let (manual_path, manual_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("base")).unwrap();
    state.current_snapshot_context = Some(manual_context);
    let auto_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
        .unwrap()
        .unwrap();
    let auto = read_snapshot(&auto_path);
    state.current_snapshot_context = Some(SnapshotContext::from_snapshot(
        snapshot_key_from_path(&auto_path).unwrap().to_string(),
        &auto,
    ));

    let (updated_base, _) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("base")).unwrap();
    assert_eq!(updated_base, manual_path);

    let (_, auto_context) =
        load_snapshot_by_key_in_dir(temp.path(), snapshot_key_from_path(&auto_path).unwrap())
            .unwrap();
    assert!(auto_context.is_empty());
    let auto = read_snapshot(&auto_path);
    state.current_snapshot_context = Some(SnapshotContext::from_snapshot(
        snapshot_key_from_path(&auto_path).unwrap().to_string(),
        &auto,
    ));
    let (branch_path, branch_context) =
        save_manual_snapshot_in_dir(temp.path(), &state, None, Some("branch")).unwrap();
    assert_eq!(branch_path.file_name().unwrap(), "base_branch.json");
    assert_eq!(branch_context.parent_chain, vec!["base".to_string()]);
}

#[test]
fn unnamed_manual_saves_generate_unique_names() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state_with_prompt("same second");
    let (_, first) = save_manual_snapshot_in_dir(temp.path(), &state, None, None).unwrap();
    state.current_snapshot_context = Some(first.clone());
    let (_, second) = save_manual_snapshot_in_dir(temp.path(), &state, None, None).unwrap();

    assert_ne!(first.name, second.name);
    assert!(second.name.starts_with(&format!("{}-", first.name)));
}

#[test]
fn legacy_v1_snapshots_are_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let legacy_path = temp.path().join("legacy.json");
    fs::write(
        &legacy_path,
        r#"{"version":1,"saved_at_ms":1,"session_id":"legacy","workspace":""}"#,
    )
    .unwrap();
    let before = fs::read(&legacy_path).unwrap();

    let catalog = list_session_snapshots_in_dir(temp.path()).unwrap();
    assert!(catalog.is_empty());
    assert!(load_snapshot_in_dir(temp.path(), "legacy").is_err());
    let state = state_with_prompt("do not touch legacy data");
    assert!(save_manual_snapshot_in_dir(temp.path(), &state, None, Some("legacy")).is_err());
    assert_eq!(fs::read(&legacy_path).unwrap(), before);
}

#[test]
fn dialog_defaults_to_manual_and_toggles_non_empty_panes() {
    fn entry(kind: SnapshotKind, name: &str) -> SessionSnapshotListEntry {
        SessionSnapshotListEntry {
            kind,
            name: name.to_string(),
            snapshot_key: name.to_string(),
            saved_at_ms: 0,
            parent_name: None,
            parent_chain: Vec::new(),
            depth: 0,
            base_manual_name: None,
        }
    }

    let mut dialog = SessionSnapshotDialog::new(SessionSnapshotCatalog {
        manual: vec![
            entry(SnapshotKind::Manual, "manual-1"),
            entry(SnapshotKind::Manual, "manual-2"),
        ],
        automatic: vec![entry(SnapshotKind::Auto, "auto")],
    });
    assert_eq!(dialog.active_pane, SessionSnapshotPane::Manual);
    dialog.move_down();
    assert_eq!(dialog.selected_entry().unwrap().name, "manual-2");
    for _ in 0..10 {
        dialog.move_down();
    }
    assert_eq!(dialog.manual_selected, 1);
    dialog.toggle_pane();
    assert_eq!(dialog.active_pane, SessionSnapshotPane::Automatic);
    assert_eq!(dialog.selected_entry().unwrap().name, "auto");

    let automatic_only = SessionSnapshotDialog::new(SessionSnapshotCatalog {
        manual: Vec::new(),
        automatic: vec![entry(SnapshotKind::Auto, "auto")],
    });
    assert_eq!(automatic_only.active_pane, SessionSnapshotPane::Automatic);
}
