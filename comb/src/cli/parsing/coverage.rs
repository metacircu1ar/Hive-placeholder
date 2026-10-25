use super::*;
#[path = "../../../../tests/support/arguments.rs"]
mod inventory;
mod negative;
mod numeric;

fn example(path: &str) -> Vec<OsString> {
    let path = if path == "@global" { "append" } else { path };
    let text = catalog::examples(path).unwrap();
    let mut argv: Vec<OsString> = text.as_slice()[0]
        .split_whitespace()
        .map(Into::into)
        .collect();
    // Global options may precede the command, so locate its actual word sequence.
    let words: Vec<OsString> = path.split_whitespace().map(Into::into).collect();
    let start = argv
        .windows(words.len())
        .position(|part| part == words)
        .unwrap();
    argv.insert(start + words.len(), "<positionals>".into());
    argv
}

#[test]
fn every_public_value_argument_has_a_negative_case_or_deferred_reason() {
    let mut command = schema();
    command.build();
    inventory::check(&command, negative::ROWS, fixture, |argv| {
        let exit = parse(argv).err().expect("invalid value");
        assert_eq!(exit.exit_code, 2, "{argv:?}: {}", exit.stderr);
        serde_json::from_str::<serde_json::Value>(&exit.stdout).unwrap()["error"].clone()
    });
}

#[test]
fn cli_and_wire_enums_have_one_spelling() {
    inventory::enum_spelling::<comb::search::Mode>();
    inventory::enum_spelling::<comb::page::Decision>();
}

fn fixture(path: &str, arg: &clap::Arg) -> Vec<OsString> {
    if path == "import" && arg.get_id() == "abandon" {
        return ["comb", "import"].map(Into::into).to_vec();
    }
    let mut argv = example(path);
    if path == "setup" && arg.get_id() == "reviewer_role" {
        argv.push("--for-hive".into());
    }
    if path == "setup" && arg.get_id() == "service_path" {
        argv.retain(|value| value != "--no-services");
    }
    argv
}
