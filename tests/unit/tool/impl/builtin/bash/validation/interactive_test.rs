use super::*;

fn validate(command: &str) -> ValidationResult {
    validate_interactive_command(&BashInput {
        command: command.to_string(),
        cwd: None,
        timeout: None,
    })
}

/// ssh with neither a key nor hostkey handling: the rejection message must
/// walk the model through a structured ask_user_question flow.
#[test]
fn ssh_without_key_guides_through_ask_user_question_flow() {
    let result = validate("ssh root@192.168.1.1 ls /root");
    assert!(!result.result);
    assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND));

    let message = result.message.unwrap();
    assert!(message.contains("❌ 不支持交互式 bash 命令"));
    assert!(message.contains("命令类型：ssh"));
    assert!(message.contains("原始命令：ssh root@192.168.1.1 ls /root"));
    // The improved flow must direct the model through a structured ask_user_question
    // flow rather than asking for password/keyfile directly.
    assert!(message.contains("ask_user_question"));
    // issue 2: the flow must first let the user choose an auth method via a
    // `choice` question, not ask for password/keyfile directly.
    assert!(message.contains("选择登录方式"));
    assert!(message.contains("choice 问题让用户选择登录方式"));
    assert!(message.contains("不要直接问密码/密钥"));
    assert!(message.contains("密钥登录"));
    assert!(message.contains("密码登录"));
    assert!(message.contains("sshpass"));
    // issue 1: username/host/keyfile-path are not sensitive; only the password
    // may be hidden. The guidance must make this explicit.
    assert!(message.contains("is_secret"));
    assert!(message.contains("is_secret 只能用于密码"));
    assert!(message.contains("用户名、主机、密钥文件路径均非敏感信息"));
    assert!(message.contains("切勿隐藏用户名"));
    // issue 3: when the user chooses key-based login but does not provide a
    // keyfile, the guidance must instruct using -o BatchMode=yes so ssh
    // automatically tries default keys (~/.ssh/id_rsa, id_ed25519, …).
    assert!(message.contains("BatchMode=yes"));
    assert!(message.contains("自动"));
    assert!(message.contains("默认密钥"));
    assert!(message.contains("自动寻找默认密钥"));
}

/// Key-based auth already configured (via `-i <key>`, compact `-i<key>`, or
/// `-o IdentityFile=<key>`): no user interaction is needed — only hostkey
/// handling is missing, so the message must point at that instead of asking
/// for a password.
#[test]
fn key_based_ssh_without_hostkey_gets_hostkey_only_guidance() {
    // (command, extra substrings the guidance must mention)
    let cases = [
        (
            "ssh -i ~/.ssh/id_rsa root@192.168.1.1 ls /root",
            vec![
                "无需询问用户",
                "StrictHostKeyChecking=accept-new",
                "BatchMode=yes",
            ],
        ),
        // SSH allows the -i option to be written compactly without separating
        // whitespace, e.g. `ssh -i~/.ssh/id_rsa ...`. This must still be
        // recognized as key-based auth (no password needed).
        ("ssh -i~/.ssh/id_rsa root@192.168.1.1 ls /root", vec![]),
        // `-o IdentityFile=<key>` is equivalent to `-i <key>`.
        (
            "ssh -o IdentityFile=~/.ssh/id_rsa root@192.168.1.1 ls /root",
            vec!["无需询问用户"],
        ),
    ];
    for (command, extra) in cases {
        let result = validate(command);
        assert!(!result.result, "{command}");
        let message = result.message.unwrap();
        assert!(message.contains("已使用密钥登录"), "{command}");
        // Password should not be requested since key-based auth is configured.
        assert!(!message.contains("密码"), "{command}");
        for expected in &extra {
            assert!(message.contains(*expected), "{command}");
        }
    }
}

/// Hostkey handling is configured, but without a key the command still needs
/// a password: the rejection must say so (and not ask for hostkey
/// confirmation).
#[test]
fn hostkey_configured_ssh_without_key_still_needs_password() {
    for command in [
        "ssh -o StrictHostKeyChecking=no root@192.168.1.1 ls /root",
        "ssh -o StrictHostKeyChecking=accept-new root@192.168.1.1 ls /root",
    ] {
        let result = validate(command);
        assert!(!result.result, "{command}");
        let message = result.message.unwrap();
        assert!(message.contains("密码"), "{command}");
        assert!(!message.contains("主机密钥确认"), "{command}");
    }
}

/// Command forms that are fully non-interactive must be allowed.
#[test]
fn non_interactive_command_forms_are_allowed() {
    let cases = [
        // ssh: key (separate and compact -i forms) + hostkey handling.
        "ssh -i ~/.ssh/id_rsa -o StrictHostKeyChecking=no root@192.168.1.1 ls /root",
        "ssh -i~/.ssh/id_rsa -o StrictHostKeyChecking=no root@192.168.1.1 ls /root",
        // BatchMode=yes disables both password prompts AND host key
        // confirmation requests per ssh_config, so the command is fully
        // non-interactive (it will fail cleanly rather than hang if auth
        // cannot complete).
        "ssh -o BatchMode=yes -o StrictHostKeyChecking=no root@192.168.1.1 ls /root",
        "ssh -o BatchMode=yes root@192.168.1.1 ls /root",
        // `-o IdentityFile=<key>` is equivalent to `-i <key>`.
        "ssh -o IdentityFile=~/.ssh/id_rsa -o StrictHostKeyChecking=no root@192.168.1.1 ls /root",
        // StrictHostKeyChecking=accept-new auto-adds new hosts without
        // prompting, so no hostkey confirmation is needed.
        "ssh -i ~/.ssh/id_rsa -o StrictHostKeyChecking=accept-new root@192.168.1.1 ls /root",
        // sudo -n never prompts for a password.
        "sudo -n cat /var/log/syslog",
        // mysql with an inline password value.
        "mysql -u root -p'mypassword'",
        // gpg with batch mode.
        "gpg --batch --passphrase-fd 0 --decrypt file.gpg",
        // sshpass feeds the password non-interactively.
        "sshpass -p 'password' ssh root@192.168.1.1 ls /root",
        // Ordinary commands never match an interactive rule.
        "ls -la",
        // Empty command won't be detected as interactive.
        "",
    ];
    for command in cases {
        assert!(validate(command).result, "should be allowed: {command}");
    }
}

/// Commands that require interaction (password/passphrase prompts or hostkey
/// confirmation) must be rejected with INTERACTIVE_COMMAND. scp and
/// rsync-over-ssh go through the ssh rule.
#[test]
fn interactive_commands_are_rejected() {
    // (command, substring the message must mention)
    let cases: [(&str, Option<&str>); 7] = [
        ("passwd", None),
        ("su -", None),
        ("mysql -u root -p", None),
        ("gpg --decrypt file.gpg", None),
        ("sudo cat /var/log/syslog", Some("sudo")),
        ("scp file.txt root@192.168.1.1:/tmp/", Some("ssh")),
        ("rsync -avz -e ssh file.txt root@192.168.1.1:/tmp/", None),
    ];
    for (command, mentions) in cases {
        let result = validate(command);
        assert!(!result.result, "should be rejected: {command}");
        assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND), "{command}");
        if let Some(expected) = mentions {
            assert!(result.message.unwrap().contains(expected), "{command}");
        }
    }
}
