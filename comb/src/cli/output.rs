//! Checked runtime output. MCP never acquires the ordinary stdout channel.
#[cfg(test)]
mod tests;

use super::input::Result;
use super::{Exit, parsing};
use hive_input::Rendered;
use std::io::Write;

pub(crate) fn finish(result: Result<Exit>, mcp: bool) -> u8 {
    if mcp {
        return emit_mcp(result, &mut std::io::stderr().lock());
    }
    emit_to(
        result,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

fn emit_to(result: Result<Exit>, stdout: &mut impl Write, stderr: &mut impl Write) -> u8 {
    let suppressed = matches!(&result, Ok(Exit::Suppressed(_)));
    let output = render(result);
    let code = parsing::emit_to(&output, stdout, stderr);
    // The failure is durably recorded. A broken output pipe must not turn it
    // back into a restart loop under the native service manager.
    if suppressed { 0 } else { code }
}

fn render(result: Result<Exit>) -> Rendered {
    let (stdout, stderr, exit_code) = match result {
        Ok(Exit::Json(value) | Exit::Suppressed(value)) => (format!("{value}\n"), String::new(), 0),
        Ok(Exit::Text(text)) => (text, String::new(), 0),
        Ok(Exit::Silent) => (String::new(), String::new(), 0),
        Err(error) => return error.render(),
    };
    Rendered {
        stdout,
        stderr,
        exit_code,
    }
}

fn emit_mcp(result: Result<Exit>, stderr: &mut impl Write) -> u8 {
    match result {
        Ok(Exit::Silent) => 0,
        Err(error) => {
            let output = error.render_mcp();
            match parsing::channel(stderr, &output.stderr) {
                Ok(()) => output.exit_code,
                Err(_) => 1,
            }
        }
        Ok(_) => 1, // An ordinary CLI result must never become an MCP frame.
    }
}
