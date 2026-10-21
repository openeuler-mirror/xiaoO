use super::expand_tilde;

#[test]
fn bare_tilde_expands_to_home() {
    let home = std::env::var_os("HOME").expect("HOME set in test environment");
    assert_eq!(expand_tilde("~"), Some(std::path::PathBuf::from(home)));
}

#[test]
fn tilde_slash_joins_suffix() {
    let home = std::env::var_os("HOME").expect("HOME set in test environment");
    assert_eq!(
        expand_tilde("~/bin/tool"),
        Some(std::path::PathBuf::from(home).join("bin/tool"))
    );
}

#[test]
fn non_tilde_values_return_none() {
    assert_eq!(expand_tilde("/usr/bin/env"), None);
    assert_eq!(expand_tilde("./local"), None);
    assert_eq!(expand_tilde("command"), None);
    assert_eq!(expand_tilde("~user/bin"), None);
    assert_eq!(expand_tilde(""), None);
}
