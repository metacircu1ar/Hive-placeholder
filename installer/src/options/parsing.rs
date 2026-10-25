//! Parser failures use trusted metadata; arbitrary argv is never echoed.
use super::{Args, input};
use clap::{CommandFactory, FromArgMatches};
use hive_input::{
    Examples, InputError, Product, Rendered,
    clap::{Context, Decision},
};
use std::{ffi::OsString, io::Write};

pub(super) const EXAMPLES: &[&str] = &[
    "hive-install --from /private/assets --prefix /private/hive --comb --no-setup",
    "hive-install --prefix /private/hive --uninstall --yes",
    "hive-install --prefix /private/hive --uninstall --purge --dry-run",
];

pub(super) fn parse(args: Vec<OsString>) -> Result<Args, Rendered> {
    let examples =
        Examples::new(EXAMPLES.iter().map(|s| (*s).into()).collect()).map_err(|_| operational())?;
    let mut schema = Args::command()
        .bin_name("hive-install")
        .after_help(examples.help());
    let matches = match schema.try_get_matches_from_mut(args) {
        Ok(matches) => matches,
        Err(error) => {
            let context = Context::new(&schema, &examples, &[]).map_err(|_| operational())?;
            return Err(match context.convert(&error) {
                Ok(Decision::Input(error)) => render(&error),
                Ok(Decision::Help | Decision::Version) => Rendered {
                    stdout: error.render().to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                },
                _ => operational(),
            });
        }
    };
    let mut args = Args::from_arg_matches(&matches).map_err(|_| operational())?;
    input::prepare(&mut args).map_err(|error| match error.downcast_ref::<InputError>() {
        Some(error) => render(error),
        None => Rendered {
            stdout: String::new(),
            stderr: format!("hive install: {error}\n"),
            exit_code: 1,
        },
    })?;
    Ok(args)
}
fn render(error: &InputError) -> Rendered {
    error
        .render(Product::Hive)
        .unwrap_or_else(|_| operational())
}
fn operational() -> Rendered {
    Rendered {
        stdout: String::new(),
        stderr: "hive install: Cannot process command-line input.\n".into(),
        exit_code: 1,
    }
}
pub(crate) fn emit(output: &Rendered) -> u8 {
    let out = std::io::stdout().lock().write_all(output.stdout.as_bytes());
    let err = std::io::stderr().lock().write_all(output.stderr.as_bytes());
    if out.is_err() || err.is_err() {
        1
    } else {
        output.exit_code
    }
}
