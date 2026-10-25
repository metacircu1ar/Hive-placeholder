//! Adapt parser errors using the selected command's trusted schema and metadata.
//!
//! This module does not parse argv, select commands, print errors or exit. The
//! binary remains the sole owner of parsing, command selection and output.
mod fields;
pub mod parsers;
#[cfg(test)]
mod tests;

use crate::{Code, Examples, InputError, InvalidMetadata, error::metadata, validate::Violation};
use ::clap::{
    Arg, Command, Error,
    error::{ContextKind, ContextValue, ErrorKind},
};
use std::error::Error as _;

/// Echo is opt-in per schema argument, never inferred from a field name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Received {
    Omit,
    Public,
}

pub struct FieldRule<'a> {
    pub id: &'a str,
    pub expected: &'a str,
    pub received: Received,
}

/// Separate successful display and operational parser failure from invalid input.
#[derive(Debug)]
pub enum Decision {
    Input(InputError),
    Help,
    Version,
    Operational,
}

/// The caller supplies the selected built Command, not a root guessing at a child.
pub struct Context<'a> {
    command: &'a Command,
    examples: &'a Examples,
    rules: &'a [FieldRule<'a>],
}
impl<'a> Context<'a> {
    pub fn new(
        command: &'a Command,
        examples: &'a Examples,
        rules: &'a [FieldRule<'a>],
    ) -> Result<Self, InvalidMetadata> {
        for (index, rule) in rules.iter().enumerate() {
            if !command
                .get_arguments()
                .any(|arg| arg.get_id().as_str() == rule.id)
                || rules[..index].iter().any(|prior| prior.id == rule.id)
            {
                return Err(InvalidMetadata::Field);
            }
            if !metadata(rule.expected, 1024) {
                return Err(InvalidMetadata::Expected);
            }
        }
        Ok(Self {
            command,
            examples,
            rules,
        })
    }

    pub fn convert(&self, error: &Error) -> Result<Decision, InvalidMetadata> {
        let (mut code, message) = match error.kind() {
            ErrorKind::DisplayHelp => return Ok(Decision::Help),
            ErrorKind::DisplayVersion => return Ok(Decision::Version),
            ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => (
                Code::MissingArgument,
                "A required command or argument is missing.",
            ),
            ErrorKind::MissingRequiredArgument | ErrorKind::TooFewValues => (
                Code::MissingArgument,
                "A required argument or value is missing.",
            ),
            ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => {
                (Code::UnknownArgument, "Unknown argument or command.")
            }
            ErrorKind::ArgumentConflict => (
                Code::ConflictingArguments,
                "These arguments cannot be combined.",
            ),
            ErrorKind::NoEquals => (Code::InvalidValue, "Use '=' to assign this option's value."),
            ErrorKind::TooManyValues | ErrorKind::WrongNumberOfValues => {
                (Code::InvalidValue, "Wrong number of argument values.")
            }
            ErrorKind::InvalidValue | ErrorKind::ValueValidation => {
                (Code::InvalidValue, "Invalid argument value.")
            }
            ErrorKind::InvalidUtf8 => (
                Code::InvalidValue,
                "The argument must contain valid UTF-8 text.",
            ),
            // I/O, formatting and unclassified future kinds are operational.
            _ => return Ok(Decision::Operational),
        };
        // Unknown tokens may happen to equal a schema ID; they do not identify it.
        let argument = if matches!(
            error.kind(),
            ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand
        ) {
            None
        } else if error.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand {
            fields::required(self.command)
        } else {
            fields::unique(self.command, error.get(ContextKind::InvalidArg))
        };
        let rule = argument.and_then(|arg| self.rule(arg));
        let field = argument.map_or_else(|| self.slot(), fields::name);
        let mut expected = self.expected(error, argument, rule);
        // Only shared validators can construct this type. Never forward any other
        // parser source, even when its Display text looks like a validation error.
        if error.kind() == ErrorKind::ValueValidation
            && let Some(violation) = error
                .source()
                .and_then(|source| source.downcast_ref::<Violation>())
        {
            code = violation.code();
            violation.expected().clone_into(&mut expected);
        }
        let mut message = message.to_owned();
        if let Some(suggestion) = fields::suggestion(self.command, argument, error) {
            message.push_str(" Did you mean '");
            message.push_str(&suggestion);
            message.push_str("'?");
        }
        let mut input = InputError::new(
            code,
            message,
            field,
            expected,
            self.examples.as_slice().to_vec(),
        )?;
        if rule.is_some_and(|rule| rule.received == Received::Public)
            && let Some(ContextValue::String(value)) = error.get(ContextKind::InvalidValue)
        {
            input = input.with_public_received(value);
        }
        Ok(Decision::Input(input))
    }

    fn rule(&self, argument: &Arg) -> Option<&FieldRule<'_>> {
        self.rules
            .iter()
            .find(|rule| rule.id == argument.get_id().as_str())
    }

    fn slot(&self) -> String {
        if self.command.get_subcommands().next().is_some() {
            "subcommand".into()
        } else {
            "arguments".into()
        }
    }

    fn expected(
        &self,
        error: &Error,
        argument: Option<&Arg>,
        rule: Option<&FieldRule<'_>>,
    ) -> String {
        if error.kind() == ErrorKind::ArgumentConflict {
            if let Some(prior) = fields::unique(self.command, error.get(ContextKind::PriorArg)) {
                return format!("do not combine this argument with {}", fields::name(prior));
            }
            return "a non-conflicting set of arguments".into();
        }
        for kind in [ContextKind::ExpectedNumValues, ContextKind::MinValues] {
            if let Some(ContextValue::Number(count)) = error.get(kind)
                && *count >= 0
            {
                let quantifier = if kind == ContextKind::MinValues {
                    "at least"
                } else {
                    "exactly"
                };
                return format!("{quantifier} {count} values");
            }
        }
        if let Some(rule) = rule {
            return rule.expected.into();
        }
        if let Some(arg) = argument {
            let values: Vec<_> = arg
                .get_possible_values()
                .into_iter()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_owned())
                .collect();
            if !values.is_empty() {
                return format!("one of: {}", values.join(", "));
            }
            return format!("a valid value for {}", fields::name(arg));
        }
        let commands: Vec<_> = self
            .command
            .get_subcommands()
            .filter(|cmd| !cmd.is_hide_set())
            .map(Command::get_name)
            .collect();
        if commands.is_empty() {
            "arguments matching the command's documented form".into()
        } else {
            format!("a command from: {}", commands.join(", "))
        }
    }
}
