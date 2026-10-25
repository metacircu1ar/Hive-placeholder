use crate::InputError;
use serde::Serialize;

/// Select local CLI privacy policy explicitly; wire views always omit input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Product {
    Hive,
    Comb,
}

/// Complete output for the binary's single stream owner. No I/O happens here.
#[derive(Debug, Eq, PartialEq)]
pub struct Rendered {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
}

impl InputError {
    pub fn render(&self, product: Product) -> serde_json::Result<Rendered> {
        #[derive(Serialize)]
        struct LocalError<'a> {
            #[serde(flatten)]
            wire: crate::WireError<'a>,
            #[serde(skip_serializing_if = "Option::is_none")]
            received: Option<&'a str>,
        }
        #[derive(Serialize)]
        struct Envelope<'a> {
            error: LocalError<'a>,
        }
        let error = LocalError {
            wire: self.wire(),
            received: match product {
                Product::Hive => self.received.as_deref(),
                Product::Comb => None,
            },
        };
        let mut stdout = serde_json::to_string(&Envelope { error })?;
        stdout.push('\n');
        let mut stderr = format!("{self}\n");
        if let Some(received) = &self.received {
            stderr.push_str("Received: ");
            stderr.push_str(received);
            stderr.push('\n');
        }
        stderr.push_str(&self.examples.help());
        Ok(Rendered {
            stdout,
            stderr,
            exit_code: 2,
        })
    }
}
