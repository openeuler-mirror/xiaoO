use ratatui::style::{Color, Modifier};

use super::running_status_light;

#[test]
fn running_status_light_blinks_at_human_scale() {
    let (bright_symbol, bright_style) = running_status_light(0, Color::Green);
    let (dim_symbol, dim_style) = running_status_light(700, Color::Green);

    assert_eq!(bright_symbol, "●");
    assert!(bright_style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(dim_symbol, "○");
    assert!(!dim_style.add_modifier.contains(Modifier::BOLD));
}
