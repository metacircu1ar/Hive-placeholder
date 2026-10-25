//! Never let a native login consume the curl/bootstrap input stream.
use crate::{Result, options::Args};
use std::{
    fs::OpenOptions,
    path::Path,
    process::{Command, Stdio},
};
pub fn run(prefix: &Path, args: &Args) -> Result<()> {
    if args.no_setup {
        return Ok(());
    }
    let name = if args.standalone_comb {
        "comb"
    } else {
        "hivectl"
    };
    let executable = prefix.join("bin").join(name);
    let mut command = Command::new(&executable);
    command.arg("setup").args(&args.setup);
    if !args.setup.iter().any(|a| {
        a.to_string_lossy()
            .starts_with("--preserve-stopped-services")
    }) {
        command.arg("--preserve-stopped-services");
    }
    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty");
    if (args.yes || tty.is_err())
        && !args
            .setup
            .iter()
            .any(|a| a == "--yes" || a.to_string_lossy().starts_with("--yes="))
    {
        command.arg("--yes");
    }
    match tty {
        Ok(tty) => {
            command
                .stdin(tty.try_clone()?)
                .stdout(tty.try_clone()?)
                .stderr(tty);
        }
        Err(error) => {
            tracing::info!(name: "install.setup_noninteractive",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "use_null_input");
            command.stdin(Stdio::null());
        }
    }
    let status = command.status()?;
    if !status.success() {
        return Err(format!(
            "Binaries are installed, but setup did not finish ({status}). Correct the \
            reported settings and rerun {} setup with the same options.",
            executable.display()
        )
        .into());
    }
    Ok(())
}
