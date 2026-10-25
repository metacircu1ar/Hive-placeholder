use super::*;

#[test]
fn reporting_turns_clicks_and_sgr_coordinates_on_and_off_in_reverse() {
    let mut on = String::new();
    Reporting(true).write_ansi(&mut on).unwrap();
    assert_eq!(on, "\u{1b}[?1000h\u{1b}[?1006h");
    let mut off = String::new();
    Reporting(false).write_ansi(&mut off).unwrap();
    assert_eq!(off, "\u{1b}[?1006l\u{1b}[?1000l");
}
