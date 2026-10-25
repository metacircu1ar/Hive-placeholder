use super::*;
use std::collections::BTreeSet;
type Boundary = (&'static str, &'static str, usize, usize);
const BOUNDS: &[Boundary] = &[
    ("graph", "depth", 0, 8),
    ("list", "limit", 1, 1000),
    ("links", "limit", 1, 1000),
    ("backlinks", "limit", 1, 1000),
    ("search", "limit", 1, 100),
    ("serve", "vector_cache_mib", 1, 8192),
    ("serve", "log_file_mib", 1, 1024),
    ("serve", "log_files", 2, 64),
    ("service", "log_file_mib", 1, 1024),
    ("service", "log_files", 2, 64),
    ("logs", "tail", 1, 2000),
    ("model use-http", "dimensions", 1, 4096),
    ("model use-http", "max_tokens", 16, 8192),
    ("model use-http", "timeout_seconds", 1, 120),
];

#[test]
fn every_numeric_argument_has_boundary_evidence() {
    let mut command = schema();
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
    for &(path, id, low, high) in BOUNDS {
        let arg = &arguments[&(path.to_owned(), id.to_owned())];
        let base = if path == "service" {
            example("service install")
        } else {
            example(path)
        };
        let mut base = base;
        if (path, id) == ("model use-http", "max_tokens") {
            base.extend(["--tokenizer".into(), "tokenizer.json".into()]);
        }
        for (value, valid) in [
            (low.to_string(), true),
            (high.to_string(), true),
            ((high + 1).to_string(), false),
            ("-1".to_string(), false),
        ] {
            let argv = inventory::with_value(base.clone(), arg, &value);
            assert_eq!(parse(&argv).is_ok(), valid, "{argv:?}");
        }
        if low > 0 {
            let argv = inventory::with_value(base, arg, &(low - 1).to_string());
            assert!(parse(&argv).is_err(), "{argv:?}");
        }
    }
}
