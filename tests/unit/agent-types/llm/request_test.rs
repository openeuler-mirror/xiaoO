use super::ReasoningEffort;
use std::str::FromStr;

#[test]
fn reasoning_effort_cycles_off_high_max() {
    assert_eq!(ReasoningEffort::Off.next(), ReasoningEffort::High);
    assert_eq!(ReasoningEffort::High.next(), ReasoningEffort::Max);
    assert_eq!(ReasoningEffort::Max.next(), ReasoningEffort::Off);
}

#[test]
fn reasoning_effort_parses_aliases() {
    assert_eq!(
        ReasoningEffort::from_str("none").unwrap(),
        ReasoningEffort::Off
    );
    assert_eq!(
        ReasoningEffort::from_str("xhigh").unwrap(),
        ReasoningEffort::Max
    );
}
