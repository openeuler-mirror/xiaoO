use super::management_capabilities;

/// Per-domain contract table. Each row is
/// `(id, configurable, runtime_read, runtime_write, test, actions)`:
/// a `None` flag means the original capability contract does not pin that
/// flag; the listed actions are asserted as membership (not equality —
/// domains may keep adding actions without breaking TUIs).
#[test]
fn reports_supported_and_unimplemented_management_actions_honestly() {
    let capabilities = management_capabilities();
    let cases: &[(
        &str,
        Option<bool>,
        Option<bool>,
        Option<bool>,
        Option<bool>,
        &[&str],
    )] = &[
        (
            "models",
            Some(true),
            None,
            None,
            Some(true),
            &["session_select", "test_connection", "list_catalog"],
        ),
        ("roles", None, None, None, None, &["list"]),
        (
            "agents",
            None,
            None,
            None,
            Some(true),
            &["list", "test_startup"],
        ),
        ("tools", None, None, None, None, &["list"]),
        (
            "skills",
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            &["list"],
        ),
        (
            "custom_tools",
            None,
            Some(true),
            None,
            Some(true),
            &["list", "validate", "render", "test"],
        ),
        ("hooks", Some(true), None, None, None, &["list"]),
        (
            "mcp_client",
            None,
            Some(true),
            None,
            Some(true),
            &["list", "test_connections"],
        ),
        (
            "lsp",
            None,
            Some(true),
            None,
            Some(true),
            &["list", "detect"],
        ),
        (
            "mcp_server",
            None,
            Some(true),
            None,
            Some(true),
            &["inspect", "preflight"],
        ),
        ("sandboxes", None, Some(true), None, None, &["list"]),
        (
            "memory",
            None,
            Some(true),
            None,
            Some(true),
            &[
                "inspect",
                "test_connection",
                "queue_status",
                "retry_failed_queue",
                "clear_failed_queue",
            ],
        ),
        (
            "compact",
            None,
            Some(true),
            None,
            Some(true),
            &["inspect", "validate"],
        ),
        (
            "backend",
            None,
            Some(true),
            None,
            Some(true),
            &["preflight"],
        ),
        (
            "channels",
            None,
            Some(true),
            None,
            Some(true),
            &["inspect", "preflight", "runtime_status", "test_connection"],
        ),
        (
            "http",
            None,
            Some(true),
            None,
            Some(true),
            &["inspect", "preflight"],
        ),
        (
            "trace",
            None,
            Some(true),
            None,
            Some(true),
            &["inspect", "list_recent"],
        ),
        (
            "vault",
            None,
            Some(true),
            Some(true),
            Some(true),
            &["inspect", "list_references", "rotate", "delete"],
        ),
        (
            "cron",
            None,
            Some(true),
            Some(true),
            None,
            &["runtime_status", "run_now"],
        ),
    ];

    for (domain_id, configurable, runtime_read, runtime_write, test, actions) in cases {
        let domain = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == *domain_id)
            .unwrap_or_else(|| panic!("`{domain_id}` capability"));
        for (flag, expected_flag, actual_flag) in [
            ("configurable", *configurable, domain.configurable),
            ("runtime_read", *runtime_read, domain.runtime_read),
            ("runtime_write", *runtime_write, domain.runtime_write),
            ("test", *test, domain.test),
        ] {
            if let Some(expected_flag) = expected_flag {
                assert_eq!(actual_flag, expected_flag, "`{domain_id}.{flag}`");
            }
        }
        for action in *actions {
            assert!(
                domain.actions.iter().any(|supported| supported == action),
                "`{domain_id}` must support `{action}`"
            );
        }
    }
}
