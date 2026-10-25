use super::*;
use clap::{CommandFactory, error::ErrorKind};

#[test]
fn flat_subcommand_order_and_shared_filter_positions_are_stable() {
    let command = Cli::command();
    assert_eq!(
        command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect::<Vec<_>>(),
        [
            "service",
            "setup",
            "setup-maintenance",
            "version",
            "model",
            "serve",
            "logs",
            "mcp",
            "credential",
            "init",
            "upgrade",
            "get",
            "put",
            "append",
            "capture",
            "promote",
            "consolidate",
            "decide",
            "rm",
            "list",
            "search",
            "embed",
            "links",
            "backlinks",
            "graph",
            "import",
            "sync",
            "doctor",
            "stats",
        ]
    );
    for name in ["list", "search"] {
        let command = command.find_subcommand(name).unwrap();
        let filters = command
            .get_arguments()
            .filter_map(clap::Arg::get_long)
            .filter(|name| ["type", "tag", "prefix", "after", "before"].contains(name))
            .collect::<Vec<_>>();
        assert_eq!(
            filters,
            if name == "list" {
                vec!["type", "tag", "prefix", "after"]
            } else {
                vec!["type", "tag", "prefix", "after", "before"]
            }
        );
    }
}

#[test]
fn required_arguments_and_credential_conflicts_remain_clap_errors() {
    for arguments in [
        vec!["append", "concepts/one", "Text"],
        vec!["promote", "concepts/one"],
        vec!["rm", "concepts/one"],
        vec![
            "credential",
            "create",
            "client",
            "--out",
            "client.json",
            "--reviewer-role",
            "critic",
        ],
    ] {
        let error = Cli::try_parse_from(["comb"].into_iter().chain(arguments))
            .err()
            .unwrap();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
        assert_eq!(error.exit_code(), 2);
    }
    let error = Cli::try_parse_from([
        "comb",
        "credential",
        "create",
        "client",
        "--out",
        "client.json",
        "--bridge",
        "--reviewer",
    ])
    .err()
    .unwrap();
    assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    assert_eq!(error.exit_code(), 2);
}

#[test]
fn flattened_link_and_filter_defaults_preserve_valid_cursor_values() {
    for command in ["links", "backlinks"] {
        let cli = Cli::try_parse_from([
            "comb",
            command,
            "concepts/one",
            "--after",
            "concepts/cursor",
        ])
        .unwrap();
        let (RawCommand::Links(args) | RawCommand::Backlinks(args)) = cli.command else {
            panic!("links");
        };
        assert_eq!(args.slug, "concepts/one");
        assert_eq!(args.after.as_deref(), Some("concepts/cursor"));
        assert_eq!(args.limit, 100);
    }
    let cli = Cli::try_parse_from(["comb", "search", "text"]).unwrap();
    let RawCommand::Search(args) = cli.command else {
        panic!("search");
    };
    assert_eq!(args.mode, Mode::Hybrid);
    assert_eq!(args.limit, comb::search::DEFAULT_SEARCH_LIMIT);
    assert!(
        args.filters.kind.is_none() && args.filters.tag.is_none() && args.filters.prefix.is_none()
    );
}

#[test]
fn guide_covers_setup_and_model_flags() {
    fn check(command: &clap::Command, guide: &str) {
        for arg in command
            .get_arguments()
            .filter(|a| !a.is_hide_set() && !a.is_global_set())
        {
            if let Some(long) = arg.get_long() {
                assert!(
                    guide
                        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                        .any(|word| word == format!("--{long}")),
                    "Undocumented {} --{long}",
                    command.get_name()
                );
            }
        }
        for sub in command.get_subcommands().filter(|c| !c.is_hide_set()) {
            check(sub, guide);
        }
    }
    let command = Cli::command();
    let guide = include_str!("../../../../docs/setup.md");
    for name in ["setup", "model"] {
        check(command.find_subcommand(name).unwrap(), guide);
    }
}

#[test]
fn cache_omission_is_distinct_from_its_explicit_default() {
    use clap::Parser;
    let RawCommand::Serve(absent) = Cli::try_parse_from(["comb", "serve"]).unwrap().command else {
        panic!()
    };
    assert_eq!(absent.vector_cache_mib, None);
    let RawCommand::Serve(explicit) =
        Cli::try_parse_from(["comb", "serve", "--vector-cache-mib", "512"])
            .unwrap()
            .command
    else {
        panic!()
    };
    assert_eq!(explicit.vector_cache_mib, Some(512));
}
