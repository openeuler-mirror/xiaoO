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
