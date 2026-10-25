//! Read prompts and interactive children through the controlling terminal only.
use super::{Result, invalid, io};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    process::{Command, Stdio},
};
pub(super) struct Terminal(Option<File>);
impl Terminal {
    pub fn open(disabled: bool) -> Self {
        Self(if disabled {
            None
        } else {
            crate::observability::optional(
                OpenOptions::new().read(true).write(true).open("/dev/tty"),
                "setup_controlling_terminal",
            )
        })
    }
    pub fn available(&self) -> bool {
        self.0.is_some()
    }
    pub fn ask(&mut self, flag: &str, question: &str, default: Option<&str>) -> Result<String> {
        let file = self
            .0
            .as_mut()
            .ok_or_else(|| invalid(format!("Noninteractive setup requires {flag}.")))?;
        write!(
            file,
            "{question} ({flag}){}: ",
            default.map_or(String::new(), |d| format!(" [{d}]"))
        )
        .map_err(io)?;
        file.flush().map_err(io)?;
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0];
            if file.read(&mut byte).map_err(io)? == 0 {
                return Err(invalid("Setup terminal closed."));
            }
            if byte[0] == b'\n' {
                break;
            }
            if bytes.len() == 4096 {
                return Err(invalid("Setup answer is too long."));
            }
            bytes.push(byte[0]);
        }
        let text = String::from_utf8(bytes)
            .map_err(|cause| invalid("Setup answers must be UTF-8.").with_source(cause))?;
        let text = text.trim();
        Ok(if text.is_empty() {
            default.unwrap_or("").into()
        } else {
            text.into()
        })
    }
}
pub(super) fn interactive(command: &mut Command) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|cause| {
            invalid(
                "Native login/install requires a controlling terminal; use --login skip and \
        install provider CLIs separately.",
            )
            .with_source(cause)
        })?;
    command
        .stdin(Stdio::from(file.try_clone().map_err(io)?))
        .stdout(Stdio::from(file.try_clone().map_err(io)?))
        .stderr(Stdio::from(file));
    let status = crate::os::spawn(command).map_err(io)?.wait().map_err(io)?;
    if !status.success() {
        return Err(invalid(
            "Interactive setup step failed. Native authentication recovery evidence is \
                retained; rerun setup after resolving the displayed error.",
        ));
    }
    Ok(())
}
