use ::clap::{
    Arg, Command, Error,
    error::{ContextKind, ContextValue},
};

pub(super) fn unique<'a>(command: &'a Command, context: Option<&ContextValue>) -> Option<&'a Arg> {
    let mut candidates = command.get_arguments().filter(|arg| {
        let display = arg.to_string();
        strings(context)
            .any(|value| value == display || value == arg.get_id().as_str() || value == name(arg))
    });
    let matched = candidates.next()?;
    if candidates.next().is_some() {
        None
    } else {
        Some(matched)
    }
}

pub(super) fn name(argument: &Arg) -> String {
    if let Some(long) = argument.get_long() {
        format!("--{long}")
    } else if let Some(short) = argument.get_short() {
        format!("-{short}")
    } else {
        argument.get_value_names().map_or_else(
            || argument.get_id().as_str().to_uppercase(),
            |names| {
                names
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            },
        )
    }
}

pub(super) fn required(command: &Command) -> Option<&Arg> {
    let mut arguments = command.get_arguments().filter(|arg| arg.is_required_set());
    let first = arguments.next()?;
    if arguments.next().is_some() {
        None
    } else {
        Some(first)
    }
}

pub(super) fn suggestion(
    command: &Command,
    argument: Option<&Arg>,
    error: &Error,
) -> Option<String> {
    for value in strings(error.get(ContextKind::SuggestedArg)) {
        if let Some(argument) = command
            .get_arguments()
            .find(|arg| !arg.is_hide_set() && name(arg) == value)
        {
            return Some(name(argument));
        }
    }
    for value in strings(error.get(ContextKind::SuggestedSubcommand)) {
        if command
            .get_subcommands()
            .any(|cmd| !cmd.is_hide_set() && cmd.get_name() == value)
        {
            return Some(value.to_owned());
        }
    }
    if let Some(argument) = argument {
        for value in strings(error.get(ContextKind::SuggestedValue)) {
            if argument
                .get_possible_values()
                .iter()
                .any(|possible| !possible.is_hide_set() && possible.get_name() == value)
            {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn strings(context: Option<&ContextValue>) -> impl Iterator<Item = &str> {
    let values: &[String] = match context {
        Some(ContextValue::String(value)) => std::slice::from_ref(value),
        Some(ContextValue::Strings(values)) => values,
        _ => &[],
    };
    values.iter().map(String::as_str)
}
