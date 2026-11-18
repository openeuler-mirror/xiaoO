use super::ReasoningEffort;
use std::str::FromStr;

#[test]
fn reasoning_effort_cycles_default_off_high_max() {
    assert_eq!(ReasoningEffort::Default.next(), ReasoningEffort::Off);
    assert_eq!(ReasoningEffort::Off.next(), ReasoningEffort::High);
    assert_eq!(ReasoningEffort::High.next(), ReasoningEffort::Max);
    assert_eq!(ReasoningEffort::Max.next(), ReasoningEffort::Default);
}

#[test]
fn reasoning_effort_parses_aliases() {
    assert_eq!(
        ReasoningEffort::from_str("default").unwrap(),
        ReasoningEffort::Default
    );
    assert_eq!(
        ReasoningEffort::from_str("auto").unwrap(),
        ReasoningEffort::Default
    );
    assert_eq!(
        ReasoningEffort::from_str("none").unwrap(),
        ReasoningEffort::Off
    );
    assert_eq!(
        ReasoningEffort::from_str("xhigh").unwrap(),
        ReasoningEffort::Max
    );
}

#[test]
fn reasoning_effort_defaults_to_provider_default() {
    assert_eq!(ReasoningEffort::default(), ReasoningEffort::Default);
}

#[test]
fn reasoning_effort_display_strings_are_stable() {
    // Display contract: the TUI status bar renders `Think(Ctrl+T) <value>`
    // with these exact strings, and E2E suites assert on them. A change
    // here is a user-visible breaking change, not a refactoring detail.
    assert_eq!(ReasoningEffort::Default.as_str(), "default");
    assert_eq!(ReasoningEffort::Off.as_str(), "off");
    assert_eq!(ReasoningEffort::High.as_str(), "high");
    assert_eq!(ReasoningEffort::Max.as_str(), "max");
    // Display delegates to as_str — this is the text the status bar shows.
    assert_eq!(ReasoningEffort::Max.to_string(), "max");
}
