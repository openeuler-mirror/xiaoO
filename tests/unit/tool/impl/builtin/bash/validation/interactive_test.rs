use super::*;

#[test]
fn test_error_message_quality() {
    let input = BashInput {
        command: "ssh root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND));

    let message = result.message.unwrap();
    assert!(message.contains("❌ 不支持交互式 bash 命令"));
    assert!(message.contains("命令类型：ssh"));
    assert!(message.contains("原始命令：ssh root@192.168.1.1 ls /root"));
    // The improved flow must direct the model through a structured ask_user_question
    // flow rather than asking for password/keyfile directly.
    assert!(message.contains("ask_user_question"));
    assert!(message.contains("选择登录方式")); // issue 2: choose method first
    assert!(message.contains("密钥登录"));
    assert!(message.contains("密码登录"));
    assert!(message.contains("sshpass"));
    // issue 1: only password may be hidden, never the username
    assert!(message.contains("is_secret"));
    assert!(message.contains("切勿隐藏用户名"));
    // issue 3: when no keyfile is given, ssh should auto-try default keys
    assert!(message.contains("BatchMode=yes"));
    assert!(message.contains("自动"));
}

#[test]
fn test_ssh_with_key_needs_hostkey_guidance() {
    let input = BashInput {
        command: "ssh -i ~/.ssh/id_rsa root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    // Has key parameter, no password needed, but still needs hostkey handling.
    assert!(!result.result);
    let message = result.message.unwrap();
    // Key-based auth is already configured; no user interaction needed — just
    // add a hostkey option and retry.
    assert!(message.contains("已使用密钥登录"));
    assert!(message.contains("无需询问用户"));
    assert!(message.contains("StrictHostKeyChecking=accept-new"));
    assert!(message.contains("BatchMode=yes"));
    // Password should not be requested since key-based auth is configured.
    assert!(!message.contains("密码"));
}

#[test]
fn test_ssh_strict_hostkey_allowed() {
    let input = BashInput {
        command: "ssh -o StrictHostKeyChecking=no root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    // Has StrictHostKeyChecking=no, no hostkey needed, but still needs password (no key)
    // Should be detected as interactive (needs password)
    assert!(!result.result);
    let message = result.message.unwrap();
    assert!(message.contains("密码"));
    assert!(!message.contains("主机密钥确认"));
}

#[test]
fn test_ssh_with_compact_key_path_needs_hostkey_guidance() {
    // SSH allows the -i option to be written compactly without separating
    // whitespace, e.g. `ssh -i~/.ssh/id_rsa ...`. This should still be
    // recognized as key-based auth (no password needed), only hostkey
    // handling is missing.
    let input = BashInput {
        command: "ssh -i~/.ssh/id_rsa root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    let message = result.message.unwrap();
    assert!(message.contains("已使用密钥登录"));
    assert!(!message.contains("密码"));
}

#[test]
fn test_ssh_with_compact_key_path_and_no_hostkey_allowed() {
    // Compact `-i<path>` form combined with StrictHostKeyChecking=no should
    // be fully non-interactive and allowed.
    let input = BashInput {
        command: "ssh -i~/.ssh/id_rsa -o StrictHostKeyChecking=no root@192.168.1.1 ls /root"
            .to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result);
}

#[test]
fn test_ssh_with_key_and_no_hostkey_allowed() {
    let input = BashInput {
        command: "ssh -i ~/.ssh/id_rsa -o StrictHostKeyChecking=no root@192.168.1.1 ls /root"
            .to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    // Has both key and StrictHostKeyChecking=no, fully non-interactive
    // Should be allowed
    assert!(result.result);
}

#[test]
fn test_ssh_with_batch_and_no_hostkey_allowed() {
    let input = BashInput {
        command: "ssh -o BatchMode=yes -o StrictHostKeyChecking=no root@192.168.1.1 ls /root"
            .to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    // Has both BatchMode and StrictHostKeyChecking=no, fully non-interactive
    // Should be allowed
    assert!(result.result);
}

#[test]
fn test_ssh_batch_mode_alone_allowed() {
    // BatchMode=yes disables both password prompts AND host key confirmation
    // requests per ssh_config, so the command is fully non-interactive (it
    // will fail cleanly rather than hang if auth cannot complete).
    let input = BashInput {
        command: "ssh -o BatchMode=yes root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result);
}

#[test]
fn test_ssh_identity_file_option_allowed() {
    // `-o IdentityFile=<key>` is equivalent to `-i <key>` and should be
    // recognized as key-based auth (no password needed).
    let input = BashInput {
        command: "ssh -o IdentityFile=~/.ssh/id_rsa -o StrictHostKeyChecking=no root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result);
}

#[test]
fn test_ssh_identity_file_option_needs_hostkey() {
    // IdentityFile= clears password requirement, but without hostkey handling
    // the command may still prompt for hostkey confirmation.
    let input = BashInput {
        command: "ssh -o IdentityFile=~/.ssh/id_rsa root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    let message = result.message.unwrap();
    // Key-based auth already configured; only hostkey handling is missing.
    assert!(message.contains("已使用密钥登录"));
    assert!(message.contains("无需询问用户"));
    // Password should not be requested since key-based auth is configured.
    assert!(!message.contains("密码"));
}

#[test]
fn test_ssh_accept_new_hostkey_allowed() {
    // StrictHostKeyChecking=accept-new auto-adds new hosts without prompting,
    // so no hostkey confirmation is needed.
    let input = BashInput {
        command:
            "ssh -i ~/.ssh/id_rsa -o StrictHostKeyChecking=accept-new root@192.168.1.1 ls /root"
                .to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result);
}

#[test]
fn test_ssh_accept_new_without_key_needs_password() {
    // accept-new handles hostkey, but without a key the command still needs
    // a password (BatchMode not set).
    let input = BashInput {
        command: "ssh -o StrictHostKeyChecking=accept-new root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    let message = result.message.unwrap();
    assert!(message.contains("密码"));
    assert!(!message.contains("主机密钥确认"));
}

#[test]
fn test_ssh_no_key_guidance_uses_batchmode_for_auto_key_detection() {
    // issue 3: when the user chooses key-based login but does not provide a keyfile,
    // the guidance must instruct using -o BatchMode=yes so ssh automatically tries
    // default keys (~/.ssh/id_rsa, id_ed25519, …) instead of giving up.
    let input = BashInput {
        command: "ssh root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    let message = result.message.unwrap();
    // The "auto-detect default keys" guidance must be present.
    assert!(message.contains("BatchMode=yes"));
    assert!(message.contains("自动"));
    assert!(message.contains("默认密钥"));
    // A no-keyfile template (ssh without -i) must be shown as an option.
    assert!(message.contains("自动寻找默认密钥"));
}

#[test]
fn test_ssh_guidance_requires_method_choice_before_asking_credentials() {
    // issue 2: the flow must direct the model to first let the user choose an auth
    // method via a `choice` question, not ask for password/keyfile directly.
    let input = BashInput {
        command: "ssh root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    let message = result.message.unwrap();
    assert!(message.contains("choice 问题让用户选择登录方式"));
    assert!(message.contains("不要直接问密码/密钥"));
}

#[test]
fn test_ssh_guidance_clarifies_only_password_is_secret() {
    // issue 1: username/host/keyfile-path are not sensitive; only the password
    // may be hidden. The guidance must make this explicit.
    let input = BashInput {
        command: "ssh root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    let message = result.message.unwrap();
    assert!(message.contains("用户名、主机、密钥文件路径均非敏感信息"));
    assert!(message.contains("is_secret 只能用于密码"));
    assert!(message.contains("切勿隐藏用户名"));
}

#[test]
fn test_sudo_needs_password_detection() {
    let input = BashInput {
        command: "sudo cat /var/log/syslog".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND));
    assert!(result.message.unwrap().contains("sudo"));
}

#[test]
fn test_sudo_with_no_password_allowed() {
    let input = BashInput {
        command: "sudo -n cat /var/log/syslog".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result); // Has -n parameter, allowed
}

#[test]
fn test_passwd_detection() {
    let input = BashInput {
        command: "passwd".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND));
}

#[test]
fn test_su_needs_password_detection() {
    let input = BashInput {
        command: "su -".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND));
}

#[test]
fn test_mysql_needs_password_detection() {
    let input = BashInput {
        command: "mysql -u root -p".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND));
}

#[test]
fn test_mysql_with_password_allowed() {
    let input = BashInput {
        command: "mysql -u root -p'mypassword'".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result); // Has password value, allowed
}

#[test]
fn test_gpg_needs_passphrase_detection() {
    let input = BashInput {
        command: "gpg --decrypt file.gpg".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(INTERACTIVE_COMMAND));
}

#[test]
fn test_gpg_with_batch_allowed() {
    let input = BashInput {
        command: "gpg --batch --passphrase-fd 0 --decrypt file.gpg".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result); // Has batch mode, allowed
}

#[test]
fn test_normal_command_allowed() {
    let input = BashInput {
        command: "ls -la".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result); // Normal command, allowed
}

#[test]
fn test_scp_detection() {
    let input = BashInput {
        command: "scp file.txt root@192.168.1.1:/tmp/".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result); // SCP needs password
    assert!(result.message.unwrap().contains("ssh"));
}

#[test]
fn test_rsync_ssh_detection() {
    let input = BashInput {
        command: "rsync -avz -e ssh file.txt root@192.168.1.1:/tmp/".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(!result.result); // Rsync with ssh needs password
}

#[test]
fn test_sshpass_command_allowed() {
    let input = BashInput {
        command: "sshpass -p 'password' ssh root@192.168.1.1 ls /root".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result); // sshpass command, non-interactive
}

#[test]
fn test_empty_command_allowed() {
    let input = BashInput {
        command: "".to_string(),
        cwd: None,
        timeout: None,
    };
    let result = validate_interactive_command(&input);
    assert!(result.result); // Empty command won't be detected as interactive
}
