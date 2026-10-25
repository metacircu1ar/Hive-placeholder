use crate::{InvalidMetadata, error::metadata};

/// Trusted command/tool examples shared by errors and command help.
///
/// The product registry supplies complete invocations. Validation bounds text;
/// it does not execute examples or try to parse shell syntax.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Examples(Vec<String>);
impl Examples {
    pub fn new(examples: Vec<String>) -> Result<Self, InvalidMetadata> {
        if !(1..=3).contains(&examples.len())
            || examples.iter().any(|example| !metadata(example, 2048))
        {
            return Err(InvalidMetadata::Examples);
        }
        Ok(Self(examples))
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    pub fn help(&self) -> String {
        let mut help = String::from("Examples:\n");
        for example in &self.0 {
            help.push_str("  ");
            help.push_str(example);
            help.push('\n');
        }
        help
    }
}
