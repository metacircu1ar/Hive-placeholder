#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Native offline installer; the POSIX bootstrap handles transport.
mod archive;
mod compatibility;
mod diagnostics;
mod lifecycle;
mod manifest;
mod options;
mod publication;
mod publish;
mod purge;
mod services;
mod setup;
mod uninstall;

use options::Args;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn execute(args: &Args) -> Result<()> {
    if args.uninstall {
        if args.purge {
            return purge::run(args);
        }
        let components = args.components();
        let removed = uninstall::run(&args.prefix()?, &components, args.yes, args.dry_run)?;
        println!(
            "{}: {} active components remain. Retained maintenance images: {:?}",
            if args.dry_run {
                "Uninstall preview"
            } else {
                "Uninstalled selected components"
            },
            removed.active,
            removed.binaries
        );
        return Ok(());
    }
    let target = manifest::native_target()?;
    let prefix = args.prefix()?;
    let parts = args.parts();
    let source = args
        .staged_from
        .as_ref()
        .or(args.source.as_ref())
        .map(std::path::PathBuf::as_path)
        .ok_or("--from is required for installation")?;
    let staged = tempfile::tempdir()?;
    let mut manifests = Vec::new();
    for part in parts {
        manifests.push(
            archive::unpack(source, staged.path(), part, &target)
                .map_err(|cause| manifest::asset(format!("{part}-{target}.tar.gz"), cause))?,
        );
        tracing::info!(name: "install.archive_unpacked", count = manifests.len(), success = true);
    }
    let installed = publish::install_with(
        &prefix,
        staged.path(),
        &manifests,
        &mut services::Services::new(args.yes)?,
        |_| Ok(()),
    )?;
    println!(
        "Installed {} into {}",
        parts_description(&manifests),
        prefix.display()
    );
    if let Some(installed) = installed {
        println!(
            "Retained {} generations ({} bytes in distinct payload inodes).",
            installed.generations, installed.bytes
        );
    } else {
        println!(
            "Installation completed, but retained-image usage could not be measured. \
                Inspect the installation log."
        );
    }
    println!("Old generations are retained; automatic pruning is not enabled.");
    println!(
        "Owned services were restored to their previous running state. Manual \
            daemons require a manual restart."
    );
    println!(
        "Add {}/bin to PATH. State, logins and configuration were not modified.",
        prefix.display()
    );
    setup::run(&prefix, args)
}

fn parts_description(parts: &[manifest::Manifest]) -> String {
    parts
        .iter()
        .map(|p| p.part.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
fn main() -> std::process::ExitCode {
    let args = match Args::read() {
        Ok(args) => args,
        Err(output) => return std::process::ExitCode::from(options::parsing::emit(&output)),
    };
    if let Err(error) = diagnostics::standalone(|| execute(&args)) {
        eprintln!("hive install: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests;
