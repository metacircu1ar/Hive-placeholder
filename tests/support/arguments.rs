//! Exact public argument inventory and executable parser-negative contracts.
use clap::{Arg, Command};
use std::{any::TypeId, collections::BTreeMap, ffi::OsString};

#[derive(Clone, Copy)]
pub enum Rule {
    Reject(&'static str),
    Deferred(&'static str),
}
pub type Row = (&'static str, &'static str, Rule);

pub fn arguments(command: &Command) -> BTreeMap<(String, String), Arg> {
    fn visit(
        command: &Command,
        path: &str,
        mut globals: BTreeMap<String, String>,
        result: &mut BTreeMap<(String, String), Arg>,
    ) {
        for arg in command
            .get_arguments()
            .filter(|arg| !arg.is_hide_set() && arg.get_action().takes_values())
        {
            let owner = if arg.is_global_set() {
                globals
                    .entry(arg.get_id().to_string())
                    .or_insert_with(|| if path.is_empty() { "@global" } else { path }.to_owned())
                    .as_str()
            } else {
                path
            };
            result.insert((owner.to_owned(), arg.get_id().to_string()), arg.clone());
        }
        for child in command
            .get_subcommands()
            .filter(|child| !child.is_hide_set() && child.get_name() != "help")
        {
            visit(
                child,
                format!("{path} {}", child.get_name()).trim(),
                globals.clone(),
                result,
            );
        }
    }
    let mut result = BTreeMap::new();
    visit(command, "", BTreeMap::new(), &mut result);
    result
}

pub fn numeric(arg: &Arg) -> bool {
    let id = arg.get_value_parser().type_id();
    [
        TypeId::of::<u64>(),
        TypeId::of::<usize>(),
        TypeId::of::<u32>(),
        TypeId::of::<u16>(),
        TypeId::of::<u8>(),
        TypeId::of::<i64>(),
        TypeId::of::<i32>(),
        TypeId::of::<f32>(),
        TypeId::of::<f64>(),
    ]
    .iter()
    .any(|kind| id == *kind)
}

pub fn check(
    command: &Command,
    rows: &[Row],
    example: impl Fn(&str, &Arg) -> Vec<OsString>,
    reject: impl Fn(&[OsString]) -> serde_json::Value,
) {
    let actual = arguments(command);
    let mut declared = BTreeMap::new();
    for &(paths, ids, rule) in rows {
        for path in paths.split('|') {
            for id in ids.split_whitespace() {
                assert!(
                    declared
                        .insert((path.to_owned(), id.to_owned()), rule)
                        .is_none(),
                    "duplicate argument policy: {path} {id}"
                );
            }
        }
    }
    let missing: Vec<_> = actual
        .keys()
        .filter(|key| !declared.contains_key(*key))
        .collect();
    let stale: Vec<_> = declared
        .keys()
        .filter(|key| !actual.contains_key(*key))
        .collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "Missing: {missing:?}; stale: {stale:?}"
    );
    for ((path, id), arg) in actual {
        match declared[&(path.clone(), id)] {
            Rule::Deferred(reason) => assert!(reason.len() > 30, "{path}: {reason}"),
            Rule::Reject(value) => {
                let argv = with_value(example(&path, &arg), &arg, value);
                let error = reject(&argv);
                assert!(
                    matches!(
                        error["code"].as_str(),
                        Some(
                            "invalid_value"
                                | "out_of_range"
                                | "invalid_json"
                                | "invalid_duration"
                                | "invalid_effort"
                                | "invalid_model"
                                | "invalid_uuid"
                                | "invalid_request_id"
                                | "invalid_run_name"
                        )
                    ),
                    "scalar rejection was masked by another error: {argv:?}: {error}"
                );
                for key in ["code", "message", "field", "expected"] {
                    assert!(
                        error[key].as_str().is_some_and(|s| !s.is_empty()),
                        "{path}: {error}"
                    );
                }
                assert!(
                    error["examples"].as_array().is_some_and(|a| !a.is_empty()),
                    "{error}"
                );
                let field = arg.get_long().map_or_else(
                    || arg.get_id().as_str().to_uppercase(),
                    |long| format!("--{long}"),
                );
                assert_eq!(error["field"], field, "{path}: {argv:?}");
            }
        }
    }
}

pub fn with_value(mut argv: Vec<OsString>, arg: &Arg, value: &str) -> Vec<OsString> {
    if let Some(long) = arg.get_long() {
        let flag = format!("--{long}");
        if let Some(index) = argv.iter().position(|v| *v == flag.as_str()) {
            if argv
                .get(index + 1)
                .is_some_and(|next| next.to_str().is_some_and(|next| !next.starts_with("--")))
            {
                argv.remove(index + 1);
            }
            argv[index] = format!("{flag}={value}").into();
        } else if let Some(index) = argv.iter().position(|v| {
            v.to_str()
                .is_some_and(|v| v.starts_with(&format!("{flag}=")))
        }) {
            argv[index] = format!("{flag}={value}").into();
        } else {
            argv.push(format!("{flag}={value}").into());
        }
    } else {
        // Examples start with executable + command path, then positional values.
        let index = arg
            .get_index()
            .unwrap_or_else(|| panic!("positional index"));
        // The caller supplies position offsets separately via an internal marker.
        let marker = argv
            .iter()
            .position(|v| v == "<positionals>")
            .unwrap_or_else(|| panic!("positional marker"));
        argv.remove(marker);
        let target = marker + index - 1;
        if target < argv.len() {
            argv[target] = value.into();
        } else {
            argv.push(value.into());
        }
    }
    argv.retain(|v| v != "<positionals>");
    argv
}

pub fn enum_spelling<T: clap::ValueEnum + serde::Serialize>() {
    for value in T::value_variants() {
        let possible = value
            .to_possible_value()
            .unwrap_or_else(|| panic!("hidden enum variant"));
        let wire = serde_json::to_value(value)
            .unwrap_or_else(|error| panic!("enum serialization: {error}"));
        assert_eq!(wire, possible.get_name());
        assert_eq!(possible.get_name_and_aliases().count(), 1, "enum alias");
    }
}
