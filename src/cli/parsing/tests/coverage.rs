use super::*;
#[path = "../../../../tests/support/arguments.rs"]
mod inventory;

mod negative;
mod numeric;
use negative::ROWS;

fn example(path: &str) -> Vec<OsString> {
    let path = match path {
        "@global" => "auth login",
        "collectors" => "collectors discard",
        path => path,
    };
    let example = catalog::examples(path).unwrap();
    let mut argv: Vec<OsString> = example.as_slice()[0]
        .split_whitespace()
        .map(Into::into)
        .collect();
    argv.insert(path.split_whitespace().count() + 1, "<positionals>".into());
    argv
}

#[test]
fn every_public_value_argument_has_a_negative_case_or_deferred_reason() {
    let mut command = schema(false);
    command.build();
    inventory::check(
        &command,
        ROWS,
        |path, _| example(path),
        |argv| {
            let Err(exit) = parse(argv, false) else {
                panic!("accepted invalid argv: {argv:?}")
            };
            assert_eq!(exit.code, 2, "{argv:?}: {}", exit.stderr);
            serde_json::from_str::<serde_json::Value>(&exit.stdout).unwrap()["error"].clone()
        },
    );
}

#[test]
fn daemon_value_arguments_use_the_same_negative_contract() {
    let mut command = schema(true);
    command.build();
    let rows: Vec<_> = ROWS
        .iter()
        .copied()
        .filter(|row| row.0 == "@global")
        .collect();
    inventory::check(
        &command,
        &rows,
        |_, _| vec!["hive-daemon".into()],
        |argv| {
            let exit = parse(argv, true).err().expect("invalid daemon value");
            assert_eq!(exit.code, 2, "{}", exit.stderr);
            serde_json::from_str::<serde_json::Value>(&exit.stdout).unwrap()["error"].clone()
        },
    );
}

#[test]
fn cli_and_wire_enums_have_one_spelling() {
    inventory::enum_spelling::<crate::config::Provider>();
    inventory::enum_spelling::<crate::config::Sandbox>();
    inventory::enum_spelling::<crate::config::Lifetime>();
    inventory::enum_spelling::<crate::runtime::domain::DeliveryMode>();
    #[cfg(feature = "comb")]
    {
        inventory::enum_spelling::<crate::runtime::collectors::EvidenceKind>();
        inventory::enum_spelling::<crate::runtime::collectors::Verdict>();
    }
}
