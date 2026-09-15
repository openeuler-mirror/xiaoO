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
