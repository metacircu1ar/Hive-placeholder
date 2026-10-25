//! Record checkout provenance; archives without Git explicitly have no commit.
#[path = "src/identity/source.rs"]
mod source;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").ok_or("Cargo manifest directory is unavailable")?,
    );
    let root = manifest
        .parent()
        .ok_or("Workspace directory is unavailable")?;
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/identity/source.rs");
    // Watch the workspace Git entry even when absent, so adding a checkout cannot
    // retain an archive's unknown identity in the Cargo cache.
    let entry = root.join(".git");
    if !entry.try_exists()? || entry.metadata()?.is_file() {
        println!("cargo:rerun-if-changed={}", entry.display());
    }
    match source::inspect(root)? {
        Some(identity) => {
            for path in identity.watch {
                println!("cargo:rerun-if-changed={}", path.display());
            }
            println!("cargo:rustc-env=HIVE_SOURCE_COMMIT={}", identity.commit);
        }
        None => println!("cargo:rustc-env=HIVE_SOURCE_COMMIT="),
    }
    Ok(())
}
