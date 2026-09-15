//! Interactive command detection for bash tool.
//!
//! This module detects interactive bash commands (SSH, sudo, passwd, etc.) that require
//! user input, and returns detailed error messages guiding the model to use ask_user_question
//! tool instead of directly executing interactive commands.

use lazy_static::lazy_static;
use regex::Regex;

use super::super::input::BashInput;
use super::backend::error_code::INTERACTIVE_COMMAND;
use super::backend::ValidationResult;

/// Interactive command detection rule
struct InteractiveCommandRule {
    /// Command type (e.g., "ssh", "sudo")
    command_type: &'static str,
    /// Pre-compiled regex patterns to match the command
    patterns: Vec<&'static Regex>,
    /// Function to check if password is needed
    needs_password_check: fn(&str) -> bool,
    /// Function to check if hostkey confirmation is needed (optional)
    needs_hostkey_check: Option<fn(&str) -> bool>,
}

// Lazy-static regex patterns for better performance
lazy_static! {
    // SSH patterns
    static ref SSH_PATTERN: Regex = Regex::new(r"^ssh\b").unwrap();
    static ref SCP_PATTERN: Regex = Regex::new(r"^scp\b").unwrap();
    static ref RSYNC_SSH_PATTERN: Regex = Regex::new(r"^rsync\b.*ssh").unwrap();
    // Key-based auth indicators: `-i <key>` or `-o IdentityFile=<key>`
    static ref SSH_KEY_PARAM: Regex = Regex::new(r"\s+-i(?:\s|[^\s-])").unwrap();
    static ref SSH_IDENTITY_FILE_OPTION: Regex = Regex::new(r"-o\s+IdentityFile=").unwrap();
    static ref SSH_BATCH_MODE: Regex = Regex::new(r"-o\s+BatchMode=yes").unwrap();
    static ref SSH_STRICT_HOSTKEY: Regex = Regex::new(r"-o\s+StrictHostKeyChecking=no").unwrap();
    static ref SSH_STRICT_HOSTKEY_ACCEPT_NEW: Regex =
        Regex::new(r"-o\s+StrictHostKeyChecking=accept-new").unwrap();

    // Sudo patterns
    static ref SUDO_PATTERN: Regex = Regex::new(r"^sudo\b").unwrap();
    static ref SUDO_NO_PASSWORD: Regex = Regex::new(r"\s+-n\b").unwrap();

    // Passwd patterns
    static ref PASSWD_PATTERN: Regex = Regex::new(r"^passwd\b").unwrap();
    static ref CHPASSWD_PATTERN: Regex = Regex::new(r"^chpasswd\b").unwrap();

    // Su patterns
    static ref SU_PATTERN: Regex = Regex::new(r"^su\b").unwrap();
    static ref SU_NO_INTERACTIVE: Regex = Regex::new(r"\s+-\s+").unwrap();

    // MySQL patterns
    static ref MYSQL_PATTERN: Regex = Regex::new(r"^mysql\b").unwrap();
    static ref MYSQLDUMP_PATTERN: Regex = Regex::new(r"^mysqldump\b").unwrap();
    static ref MYSQL_PASSWORD_PARAM: Regex = Regex::new(r"\s+-p\b").unwrap();
    static ref MYSQL_PASSWORD_VALUE: Regex = Regex::new(r"-p\s*['\w]").unwrap();

    // GPG patterns
    static ref GPG_PATTERN: Regex = Regex::new(r"^gpg\b").unwrap();
    static ref GPG_NEEDS_PASSPHRASE: Regex = Regex::new(r"--decrypt|--sign|--clearsign").unwrap();
    static ref GPG_BATCH_MODE: Regex = Regex::new(r"--batch").unwrap();

    // Predefined interactive command rules (Phase 1 + Phase 2). Held in a lazy_static
    // so the rule table can carry pre-compiled `&'static Regex` references — this avoids
    // recompiling regexes on every `detect_interactive_command` call.
    static ref INTERACTIVE_COMMAND_RULES: Vec<InteractiveCommandRule> = vec![
        // Phase 1: SSH, sudo, passwd
        InteractiveCommandRule {
            command_type: "ssh",
            patterns: vec![&*SSH_PATTERN, &*SCP_PATTERN, &*RSYNC_SSH_PATTERN],
            needs_password_check: ssh_needs_password,
            needs_hostkey_check: Some(ssh_needs_hostkey),
        },
        InteractiveCommandRule {
            command_type: "sudo",
            patterns: vec![&*SUDO_PATTERN],
            needs_password_check: sudo_needs_password,
            needs_hostkey_check: None,
        },
        InteractiveCommandRule {
            command_type: "passwd",
            patterns: vec![&*PASSWD_PATTERN, &*CHPASSWD_PATTERN],
            needs_password_check: passwd_needs_password,
            needs_hostkey_check: None,
        },
        // Phase 2: su, mysql, gpg
        InteractiveCommandRule {
            command_type: "su",
            patterns: vec![&*SU_PATTERN],
            needs_password_check: su_needs_password,
            needs_hostkey_check: None,
        },
        InteractiveCommandRule {
            command_type: "mysql",
            patterns: vec![&*MYSQL_PATTERN, &*MYSQLDUMP_PATTERN],
            needs_password_check: mysql_needs_password,
            needs_hostkey_check: None,
        },
        InteractiveCommandRule {
            command_type: "gpg",
            patterns: vec![&*GPG_PATTERN],
            needs_password_check: gpg_needs_password,
            needs_hostkey_check: None,
        },
    ];
}

/// Check if SSH command needs password.
///
/// No password is needed when key-based auth is configured (either via `-i <key>` or
/// `-o IdentityFile=<key>`), or when `BatchMode=yes` is set (which disables all password
/// prompts per ssh_config).
fn ssh_needs_password(command: &str) -> bool {
    !SSH_KEY_PARAM.is_match(command)
        && !SSH_IDENTITY_FILE_OPTION.is_match(command)
        && !SSH_BATCH_MODE.is_match(command)
}

/// Check if SSH command needs hostkey confirmation.
///
/// No hostkey prompt is needed when any of:
/// - `StrictHostKeyChecking=no` (disable hostkey verification prompt)
/// - `StrictHostKeyChecking=accept-new` (auto-add new hosts without prompting)
/// - `BatchMode=yes` (per ssh_config: disables host key confirmation requests, failing
///   cleanly instead of prompting, so the command will never hang)
///
/// Note: `UserKnownHostsFile=/dev/null` only controls where known_hosts is stored;
/// it does not by itself suppress the hostkey confirmation prompt, so it is NOT
/// treated as a suppressing condition here. It is typically combined with one of
/// the StrictHostKeyChecking options above.
fn ssh_needs_hostkey(command: &str) -> bool {
    !SSH_STRICT_HOSTKEY.is_match(command)
        && !SSH_STRICT_HOSTKEY_ACCEPT_NEW.is_match(command)
        && !SSH_BATCH_MODE.is_match(command)
}

/// Check if sudo command needs password
fn sudo_needs_password(command: &str) -> bool {
    !SUDO_NO_PASSWORD.is_match(command)
}

/// Check if passwd command needs password (always true)
fn passwd_needs_password(_command: &str) -> bool {
    true
}

/// Check if su command needs password
fn su_needs_password(command: &str) -> bool {
    !SU_NO_INTERACTIVE.is_match(command)
}

/// Check if mysql command needs password
fn mysql_needs_password(command: &str) -> bool {
    MYSQL_PASSWORD_PARAM.is_match(command) && !MYSQL_PASSWORD_VALUE.is_match(command)
}

/// Check if gpg command needs passphrase
fn gpg_needs_password(command: &str) -> bool {
    GPG_NEEDS_PASSPHRASE.is_match(command) && !GPG_BATCH_MODE.is_match(command)
}

/// Detect if command is interactive
fn detect_interactive_command(command: &str) -> Option<(String, bool, bool)> {
    let command_trimmed = command.trim();

    for rule in INTERACTIVE_COMMAND_RULES.iter() {
        // Check command patterns (pre-compiled regexes — no per-call compilation)
        for pattern in &rule.patterns {
            if pattern.is_match(command_trimmed) {
                // Check if password is needed
                let needs_password = (rule.needs_password_check)(command_trimmed);

                // Check if hostkey confirmation is needed
                let needs_hostkey = if let Some(check_fn) = rule.needs_hostkey_check {
                    check_fn(command_trimmed)
                } else {
                    false
                };

                if needs_password || needs_hostkey {
                    return Some((rule.command_type.to_string(), needs_password, needs_hostkey));
                }
            }
        }
    }
    None
}

/// Build detailed error message for interactive command
fn build_interactive_error_message(
    command_type: &str,
    needs_password: bool,
    needs_hostkey: bool,
    command: &str,
) -> String {
    let mut message_parts = vec![
        "❌ 不支持交互式 bash 命令。".to_string(),
        "".to_string(),
        format!("命令类型：{}", command_type),
        format!("原始命令：{}", command),
        "".to_string(),
    ];

    if command_type == "ssh" {
        build_ssh_guidance(&mut message_parts, needs_password);
    } else {
        build_generic_guidance(
            &mut message_parts,
            command_type,
            needs_password,
            needs_hostkey,
        );
    }

    message_parts.join("\n")
}

/// Build SSH-specific guidance directing the model to construct a non-interactive ssh
/// command, including a structured `ask_user_question` flow when auth info is missing.
///
/// Design goals (addressing common interaction pitfalls):
/// 1. Username/host/keyfile-path are NOT sensitive — only the password may be hidden.
/// 2. First let the user choose an auth method (key vs password) via a `choice` question,
///    rather than asking for password/keyfile directly.
/// 3. When key-based login is chosen but no keyfile is provided, rely on `-o BatchMode=yes`
///    so ssh automatically tries default keys (~/.ssh/id_rsa, id_ed25519, …) instead of
///    giving up.
fn build_ssh_guidance(message_parts: &mut Vec<String>, needs_password: bool) {
    if !needs_password {
        // Key-based auth already configured (via -i or -o IdentityFile=); only hostkey
        // handling is missing. No user interaction needed — just add an option and retry.
        message_parts.push(
            "命令已使用密钥登录，仅缺主机密钥处理选项，首次连接会卡在主机密钥确认。".to_string(),
        );
        message_parts.push("无需询问用户，直接在命令中补充以下任一选项后重试：".to_string());
        message_parts.push("".to_string());
        message_parts.push(
            "  ssh -i <keyfile> -o StrictHostKeyChecking=accept-new <user>@<host> <command>"
                .to_string(),
        );
        message_parts.push("  # 或 -o StrictHostKeyChecking=no（完全不验证主机密钥）".to_string());
        message_parts
            .push("  # 或 -o BatchMode=yes（禁用全部交互提示，认证失败则直接退出）".to_string());
        return;
    }

    // needs_password == true: the command lacks a non-interactive auth method. Guide the
    // model through a structured ask_user_question flow.
    message_parts.push(
        "该 ssh 命令缺少非交互式认证方式，请用 ask_user_question 按以下流程收集信息后重试："
            .to_string(),
    );
    message_parts.push("".to_string());
    message_parts
        .push("1. 若原始命令已含 <user>@<host>，直接复用，不要再问用户名/主机。".to_string());
    message_parts.push(
        "   ⚠ 用户名、主机、密钥文件路径均非敏感信息，is_secret 只能用于密码，切勿隐藏用户名。"
            .to_string(),
    );
    message_parts
        .push("2. 先用 choice 问题让用户选择登录方式（不要直接问密码/密钥）：".to_string());
    message_parts.push("   - 密钥登录（推荐）".to_string());
    message_parts.push("   - 密码登录".to_string());
    message_parts.push("3. 按所选方式收集信息（每个问题只问一件事）：".to_string());
    message_parts.push(
        "   • 密钥登录：密钥文件路径可选——用户不提供时，用 -o BatchMode=yes 让 ssh 自动"
            .to_string(),
    );
    message_parts.push("     尝试默认密钥（~/.ssh/id_rsa、id_ed25519 等），无需再问。".to_string());
    message_parts.push(
        "   • 密码登录：用 text_input + is_secret=true 收集密码（仅此项需隐藏）。".to_string(),
    );
    message_parts.push("4. 组装非交互式命令：".to_string());
    message_parts.push("   # 密钥登录（指定密钥）".to_string());
    message_parts.push(
        "   ssh -i <keyfile> -o BatchMode=yes -o StrictHostKeyChecking=accept-new <user>@<host> <command>".to_string(),
    );
    message_parts.push("   # 密钥登录（自动寻找默认密钥，不指定 keyfile 时使用）".to_string());
    message_parts.push(
        "   ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new <user>@<host> <command>"
            .to_string(),
    );
    message_parts.push("   # 密码登录".to_string());
    message_parts.push(
        "   sshpass -p '<password>' ssh -o StrictHostKeyChecking=no <user>@<host> <command>"
            .to_string(),
    );
}

/// Build generic guidance for non-SSH interactive commands (sudo, passwd, su, mysql, gpg).
fn build_generic_guidance(
    message_parts: &mut Vec<String>,
    command_type: &str,
    needs_password: bool,
    needs_hostkey: bool,
) {
    message_parts.push("需要用户提供：".to_string());
    if needs_password {
        message_parts.push("  - 密码".to_string());
    }
    if needs_hostkey {
        message_parts.push("  - 主机密钥确认".to_string());
    }

    message_parts.push("".to_string());
    message_parts.push("非交互式命令示例：".to_string());

    match command_type {
        "sudo" => {
            message_parts.push("  echo '<password>' | sudo -S <command>".to_string());
        }
        "passwd" => {
            message_parts.push("  echo '<newpass>\\n<newpass>' | passwd".to_string());
        }
        "su" => {
            message_parts.push("  echo '<password>' | su -c '<command>'".to_string());
        }
        "mysql" => {
            message_parts.push("  mysql -u <user> -p'<password>'".to_string());
        }
        "gpg" => {
            message_parts.push(
                "  echo '<passphrase>' | gpg --batch --passphrase-fd 0 --decrypt <file>"
                    .to_string(),
            );
        }
        _ => {
            message_parts.push("  请根据命令类型构造非交互式命令".to_string());
        }
    }

    message_parts.extend(vec![
        "".to_string(),
        "提示：使用 ask_user_question 时，每个问题只问一个事项，避免歧义。".to_string(),
    ]);

    // For commands that need password/passphrase, add guidance
    if needs_password {
        message_parts.push("".to_string());
        message_parts.push(
            "⚠ 收集密码/口令时用 text_input + is_secret=true 隐藏输入；用户名等非敏感信息切勿隐藏。"
                .to_string(),
        );
    }
}

/// Validate if command is interactive
pub fn validate_interactive_command(input: &BashInput) -> ValidationResult {
    let command = input.command.trim();

    // Detect if command needs interaction
    if let Some((command_type, needs_password, needs_hostkey)) = detect_interactive_command(command)
    {
        let error_message =
            build_interactive_error_message(&command_type, needs_password, needs_hostkey, command);

        return ValidationResult::error(error_message, INTERACTIVE_COMMAND);
    }

    ValidationResult::ok()
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/tool/impl/builtin/bash/validation/interactive_test.rs"]
mod tests;
