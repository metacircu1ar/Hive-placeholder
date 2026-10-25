use crate::{Config, Fields, Invalid, policy::Policy};
use std::{error::Error, fmt};
use tracing::{
    field::{Field, Visit},
    level_filters::LevelFilter,
};

pub(super) struct Visitor<'a> {
    policy: &'a Policy,
    config: &'a Config,
    trusted: bool,
    bridge: bool,
    fields: Fields,
    invalid: Option<Invalid>,
    redacted: u64,
    pub bridge_level: LevelFilter,
}
impl<'a> Visitor<'a> {
    pub fn new(policy: &'a Policy, config: &'a Config, trusted: bool, bridge: bool) -> Self {
        Self {
            policy,
            config,
            trusted,
            bridge,
            fields: Fields::default(),
            invalid: None,
            redacted: 0,
            bridge_level: LevelFilter::WARN,
        }
    }
    fn rejected(&mut self) {
        self.redacted += 1;
        if self.trusted {
            self.invalid = Some(Invalid::Name);
        }
    }
    fn accepted(&mut self, result: Result<(), Invalid>) {
        if let Err(error) = result {
            self.invalid = Some(error);
        }
    }
    pub fn finish(mut self) -> Result<Fields, Invalid> {
        if let Some(error) = self.invalid {
            return Err(error);
        }
        if self.redacted != 0 {
            self.fields.count("fields_redacted", self.redacted)?;
        }
        Ok(self.fields)
    }
}
impl Visit for Visitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        let key = field.name();
        if self.bridge && key == "log.target" {
            self.bridge_level = self.config.library_level(value);
            if let Some((directive, _)) = self.config.library_directive(value) {
                let result = self.fields.provenance("library_directive", directive);
                self.accepted(result);
            }
            self.rejected();
        } else if self.trusted && self.policy.identity.iter().any(|name| name == key) {
            let result = self.fields.identity(key, value);
            self.accepted(result);
        } else if self.trusted && self.policy.code.iter().any(|name| name == key) {
            let result = self.fields.code(key, value);
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        if self.trusted && self.policy.number.iter().any(|name| name == field.name()) {
            let result = self.fields.count(field.name(), value);
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        if self.trusted && self.policy.number.iter().any(|name| name == field.name()) {
            let result = self.fields.signed(field.name(), value);
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        if self.trusted && self.policy.flag.iter().any(|name| name == field.name()) {
            let result = self.fields.flag(field.name(), value);
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_u128(&mut self, field: &Field, value: u128) {
        if self.trusted && self.policy.number.iter().any(|name| name == field.name()) {
            let result = u64::try_from(value)
                .map_err(|_| Invalid::NumericRange)
                .and_then(|value| self.fields.count(field.name(), value));
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_i128(&mut self, field: &Field, value: i128) {
        if self.trusted && self.policy.number.iter().any(|name| name == field.name()) {
            let result = i64::try_from(value)
                .map_err(|_| Invalid::NumericRange)
                .and_then(|value| self.fields.signed(field.name(), value));
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        if self.trusted && self.policy.number.iter().any(|name| name == field.name()) {
            let result = self.fields.float(field.name(), value);
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_error(&mut self, field: &Field, value: &(dyn Error + 'static)) {
        if self.trusted && self.policy.error.iter().any(|name| name == field.name()) {
            let result = self.fields.error(field.name(), value);
            self.accepted(result);
        } else {
            self.rejected();
        }
    }
    fn record_debug(&mut self, _: &Field, _: &dyn fmt::Debug) {
        // In particular, never invoke Debug for fmt::Arguments or Display wrappers.
        self.rejected();
    }
}
