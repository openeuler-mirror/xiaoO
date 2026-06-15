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
    /// Regex patterns to match the command
    patterns: &'static [&'static str],
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
    static ref SSH_KEY_PARAM: Regex = Regex::new(r"\s+-i\s+").unwrap();
    static ref SSH_BATCH_MODE: Regex = Regex::new(r"-o\s+BatchMode=yes").unwrap();
    static ref SSH_STRICT_HOSTKEY: Regex = Regex::new(r"-o\s+StrictHostKeyChecking=no").unwrap();
    static ref SSH_USER_HOSTS_FILE: Regex = Regex::new(r"-o\s+UserKnownHostsFile=/dev/null").unwrap();

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
}

/// Check if SSH command needs password
fn ssh_needs_password(command: &str) -> bool {
    // If has key file or BatchMode, no password needed
    !SSH_KEY_PARAM.is_match(command) && !SSH_BATCH_MODE.is_match(command)
}

/// Check if SSH command needs hostkey confirmation
fn ssh_needs_hostkey(command: &str) -> bool {
    !SSH_STRICT_HOSTKEY.is_match(command) && !SSH_USER_HOSTS_FILE.is_match(command)
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

/// Predefined interactive command rules (Phase 1 + Phase 2)
const INTERACTIVE_COMMAND_RULES: &[InteractiveCommandRule] = &[
    // Phase 1: SSH, sudo, passwd
    InteractiveCommandRule {
        command_type: "ssh",
        patterns: &["^ssh\\b", "^scp\\b", "^rsync\\b.*ssh"],
        needs_password_check: ssh_needs_password,
        needs_hostkey_check: Some(ssh_needs_hostkey),
    },
    InteractiveCommandRule {
        command_type: "sudo",
        patterns: &["^sudo\\b"],
        needs_password_check: sudo_needs_password,
        needs_hostkey_check: None,
    },
    InteractiveCommandRule {
        command_type: "passwd",
        patterns: &["^passwd\\b", "^chpasswd\\b"],
        needs_password_check: passwd_needs_password,
        needs_hostkey_check: None,
    },
    // Phase 2: su, mysql, gpg
    InteractiveCommandRule {
        command_type: "su",
        patterns: &["^su\\b"],
        needs_password_check: su_needs_password,
        needs_hostkey_check: None,
    },
    InteractiveCommandRule {
        command_type: "mysql",
        patterns: &["^mysql\\b", "^mysqldump\\b"],
        needs_password_check: mysql_needs_password,
        needs_hostkey_check: None,
    },
    InteractiveCommandRule {
        command_type: "gpg",
        patterns: &["^gpg\\b"],
        needs_password_check: gpg_needs_password,
        needs_hostkey_check: None,
    },
];

/// Detect if command is interactive
fn detect_interactive_command(command: &str) -> Option<(String, bool, bool)> {
    let command_trimmed = command.trim();

    for rule in INTERACTIVE_COMMAND_RULES {
        // Check command patterns
        for pattern_str in rule.patterns {
            let pattern = Regex::new(pattern_str).unwrap();
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
        "需要用户提供：".to_string(),
    ];

    if needs_password {
        message_parts.push("  - 密码".to_string());
    }
    if needs_hostkey {
        message_parts.push("  - 主机密钥确认".to_string());
    }

    message_parts.push("".to_string());
    message_parts.push("非交互式命令示例：".to_string());

    // Generate specific guidance based on needs
    match command_type {
        "ssh" => {
            if needs_password && needs_hostkey {
                message_parts.push("  sshpass -p '<password>' ssh -o StrictHostKeyChecking=no <user>@<host> <command>".to_string());
            } else if needs_password {
                message_parts
                    .push("  sshpass -p '<password>' ssh <user>@<host> <command>".to_string());
            } else if needs_hostkey {
                message_parts
                    .push("  ssh -o StrictHostKeyChecking=no <user>@<host> <command>".to_string());
            }
        }
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
        message_parts.push("⚠ 收集密码/密钥时，请隐藏用户输入。".to_string());
    }

    message_parts.join("\n")
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
mod tests {
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
        assert!(message.contains("需要用户提供："));
        assert!(message.contains("密码"));
        assert!(message.contains("非交互式命令示例："));
        assert!(message.contains("sshpass"));
        assert!(message.contains("提示"));
        assert!(message.contains("隐藏用户输入"));
    }

    #[test]
    fn test_ssh_with_key_allowed() {
        let input = BashInput {
            command: "ssh -i ~/.ssh/id_rsa root@192.168.1.1 ls /root".to_string(),
            cwd: None,
            timeout: None,
        };
        let result = validate_interactive_command(&input);
        // Has key parameter, no password needed, but still needs hostkey confirmation (first time)
        // Should be detected as interactive (needs hostkey confirmation)
        assert!(!result.result);
        let message = result.message.unwrap();
        // Check that hostkey confirmation is listed in user interaction section
        assert!(message.contains("主机密钥确认"));
        // Since there's a key, password should not be listed
        // Check the "需要用户提供：" section
        let needs_section_start = message.find("需要用户提供：").unwrap();
        let example_section_start = message.find("非交互式命令示例：").unwrap();
        let needs_section = &message[needs_section_start..example_section_start];
        assert!(needs_section.contains("主机密钥确认"));
        assert!(!needs_section.contains("密码"));
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
}
