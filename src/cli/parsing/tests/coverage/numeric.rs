use super::*;

// Full unsigned cursor domains are deliberate: SQL adapters cap comparisons at
// i64::MAX, and output offsets are checked against the actual artifact at read.
const U64_MAX: &str = "18446744073709551615";
const U64_OVER: &str = "18446744073709551616";
const I64_MAX: &str = "9223372036854775807";
const I64_OVER: &str = "9223372036854775808";
type Boundary = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);
const BOUNDS: &[Boundary] = &[
    ("@global", "probe_timeout", "1", "300", "301"),
    ("@global", "login_timeout", "1", "3600", "3601"),
    ("agents", "after", "0", U64_MAX, U64_OVER),
    ("agents", "limit", "1", "200", "201"),
    (
        "agents",
        "stalled",
        "1s",
        "9223372036854775807s",
        "9223372036854775808s",
    ),
    ("approvals", "limit", "1", "200", "201"),
    ("approvals", "timeout", "1", "3600", "3601"),
    ("batch", "deadline_seconds", "1", I64_MAX, I64_OVER),
    ("compact", "before", "0", I64_MAX, I64_OVER),
    ("compact", "keep_per_agent", "1", I64_MAX, I64_OVER),
    (
        "gc",
        "older_than",
        "1s",
        "9223372036854775807s",
        "9223372036854775808s",
    ),
    ("logs", "tail", "1", "2000", "2001"),
    ("messages", "before", "0", U64_MAX, U64_OVER),
    ("messages", "limit", "1", "200", "201"),
    ("output", "limit", "4", "65536", "65537"),
    ("output", "offset", "0", U64_MAX, U64_OVER),
    ("read", "after", "0", U64_MAX, U64_OVER),
    ("read", "limit", "1", "200", "201"),
    ("live", "wait_ms", "0", "25000", "25001"),
    ("attach", "linger", "0", "60", "61"),
    ("panes", "limit", "1", "32", "33"),
    ("panes", "linger", "0", "60", "61"),
    ("tui", "limit", "1", "12", "13"),
    ("send", "delay", "1s", "604800s", "604801s"),
    ("send", "timeout", "1", "600", "601"),
    ("wait", "after", "0", U64_MAX, U64_OVER),
    ("wait", "timeout", "1", "3600", "3601"),
];

#[test]
fn every_numeric_argument_has_boundary_evidence() {
    let mut command = schema(false);
    command.build();
    let arguments = inventory::arguments(&command);
    let actual: BTreeSet<_> = arguments
        .iter()
        .filter(|(_, a)| inventory::numeric(a))
        .map(|(key, _)| key.clone())
        .collect();
    let declared: BTreeSet<_> = BOUNDS
        .iter()
        .map(|(p, id, ..)| (p.to_string(), id.to_string()))
        .collect();
    assert_eq!(declared.len(), BOUNDS.len(), "duplicate numeric policy");
    assert_eq!(actual, declared, "missing or stale numeric policy");
    for &(path, id, low, high, over) in BOUNDS {
        let arg = &arguments[&(path.to_owned(), id.to_owned())];
        let mut base = example(path);
        if (path, id) == ("send", "timeout") {
            base.push("--wait".into());
        }
        if (path, id) == ("attach", "linger") {
            base.push("--exit-on-end".into());
        }
        if (path, id) == ("wait", "after") {
            let pos = base.iter().position(|v| v == "--until").unwrap();
            base[pos + 1] = "turn-finished".into();
        }
        for (value, valid) in [(low, true), (high, true), (over, false), ("-1", false)] {
            let argv = inventory::with_value(base.clone(), arg, value);
            assert_eq!(parse(&argv, false).is_ok(), valid, "{argv:?}");
        }
        if low != "0" {
            let zero = if low.ends_with('s') { "0s" } else { "0" };
            let argv = inventory::with_value(base, arg, zero);
            assert!(parse(&argv, false).is_err(), "{argv:?}");
        }
    }
}
