/// Substitute argument placeholders in a skill prompt.
///
/// Supports:
/// - `$arg_name` — named argument from `arg_names`
/// - `$0`, `$1`, ... — positional arguments
/// - `$ARGUMENTS` or `${ARGUMENTS}` — full raw argument string
///
/// If no placeholders are found and args are provided,
/// appends `\n\nARGUMENTS: {args}` to the prompt.
pub fn substitute_arguments(
    prompt: &str,
    args_raw: &Option<String>,
    arg_names: &[String],
) -> String {
    let args_str = match args_raw {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return prompt.to_string(),
    };

    let positional: Vec<&str> = split_args(args_str);
    let mut result = prompt.to_string();
    let mut any_replaced = false;

    // Named arguments: $arg_name
    for (i, name) in arg_names.iter().enumerate() {
        let placeholder = format!("${}", name);
        if result.contains(&placeholder) {
            let value = positional.get(i).unwrap_or(&"");
            result = result.replace(&placeholder, value);
            any_replaced = true;
        }
    }

    // Positional: $0, $1, ...
    for (i, value) in positional.iter().enumerate() {
        let placeholder = format!("${}", i);
        if result.contains(&placeholder) {
            result = result.replace(&placeholder, value);
            any_replaced = true;
        }
    }

    // Full arguments: $ARGUMENTS / ${ARGUMENTS}
    if result.contains("$ARGUMENTS") {
        result = result.replace("$ARGUMENTS", args_str);
        any_replaced = true;
    }
    if result.contains("${ARGUMENTS}") {
        result = result.replace("${ARGUMENTS}", args_str);
        any_replaced = true;
    }

    // Fallback: append if no placeholders were found
    if !any_replaced {
        result.push_str("\n\nARGUMENTS: ");
        result.push_str(args_str);
    }

    result
}

/// Simple argument splitting: respects double-quoted strings.
fn split_args(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut chars = s.char_indices().peekable();
    let mut start = None;
    let mut in_quote = false;

    while let Some(&(i, c)) = chars.peek() {
        chars.next();
        match c {
            '"' => {
                if in_quote {
                    in_quote = false;
                } else {
                    in_quote = true;
                    if start.is_none() {
                        start = Some(i + 1);
                    }
                }
            }
            ' ' | '\t' if !in_quote => {
                if let Some(s_idx) = start.take() {
                    let end = if s.as_bytes().get(i.wrapping_sub(1)) == Some(&b'"') {
                        i - 1
                    } else {
                        i
                    };
                    result.push(&s[s_idx..end]);
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(i);
                }
            }
        }
    }

    if let Some(s_idx) = start {
        let end = if s.ends_with('"') && in_quote {
            s.len() - 1
        } else {
            s.len()
        };
        result.push(&s[s_idx..end]);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_named_args() {
        let prompt = "Review $target in $format format";
        let result = substitute_arguments(
            prompt,
            &Some("src/main.rs json".into()),
            &["target".into(), "format".into()],
        );
        assert_eq!(result, "Review src/main.rs in json format");
    }

    #[test]
    fn substitute_positional_args() {
        let prompt = "File: $0, Mode: $1";
        let result = substitute_arguments(prompt, &Some("test.rs debug".into()), &[]);
        assert_eq!(result, "File: test.rs, Mode: debug");
    }

    #[test]
    fn substitute_full_arguments() {
        let prompt = "Run with: $ARGUMENTS";
        let result = substitute_arguments(prompt, &Some("--verbose --all".into()), &[]);
        assert_eq!(result, "Run with: --verbose --all");
    }

    #[test]
    fn fallback_append() {
        let prompt = "Do something";
        let result = substitute_arguments(prompt, &Some("extra stuff".into()), &[]);
        assert_eq!(result, "Do something\n\nARGUMENTS: extra stuff");
    }

    #[test]
    fn no_args_returns_prompt_as_is() {
        let prompt = "Do something with $target";
        let result = substitute_arguments(prompt, &None, &["target".into()]);
        assert_eq!(result, prompt);
    }
}
