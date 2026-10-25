//! Separate managed state preparations from arbitrary user-selected outputs.
use std::{ffi::OsStr, io, path::Path};

/// Product-owned defaults preserve the namespace used by `Hive` callers.
pub const PUBLICATION_OPTIONS: atomic_fs::Options<'static> = (atomic_fs::Options::new())
    .with_mode(0o600)
    .with_durability(atomic_fs::Durability::Durable)
    .with_prefix(".hive-tmp-");

pub const STATE_PREFIX: &str = ".hive-tmp-state-";
pub const COMB_ADMIN_PREFIX: &str = ".comb-tmp-admin-";

/// Naming is only a candidate filter; deletion also requires writer exclusion.
pub fn state_preparation_name(name: &OsStr) -> bool {
    preparation_name(name, STATE_PREFIX)
}

pub fn comb_admin_preparation_name(name: &OsStr) -> bool {
    preparation_name(name, COMB_ADMIN_PREFIX)
}

fn preparation_name(name: &OsStr, prefix: &str) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(prefix))
        .is_some_and(|suffix| {
            (suffix.len() == 6 && suffix.bytes().all(|b| b.is_ascii_alphanumeric()))
                || (suffix.len() == 16
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        })
}

/// Apply only at output admission, never to input files or attachment basenames.
pub fn check_output_name(path: &Path) -> io::Result<()> {
    if path.file_name().is_some_and(state_preparation_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Output basename is reserved for Hive state preparations.",
        ));
    }
    if path.file_name().is_some_and(comb_admin_preparation_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Output basename is reserved for Comb administration preparations.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_only_exact_administration_basenames() {
        for suffix in ["ABC123", "0123456789abcdef"] {
            let name = format!("{COMB_ADMIN_PREFIX}{suffix}");
            assert!(comb_admin_preparation_name(OsStr::new(&name)));
            assert!(check_output_name(Path::new(&name)).is_err());
        }
        for suffix in [
            "ABCDE",
            "ABCDEFG",
            "0123456789abcdeF",
            "ABC_12",
            "ABC123.json",
        ] {
            let name = format!("{COMB_ADMIN_PREFIX}{suffix}");
            assert!(!comb_admin_preparation_name(OsStr::new(&name)));
            check_output_name(Path::new(&name)).unwrap();
        }
        check_output_name(Path::new(".comb-tmp-admin-ABC123/ordinary.json")).unwrap();
    }

    #[test]
    fn reserve_only_exact_managed_preparation_basenames() {
        for name in [".hive-tmp-state-ABC123", ".hive-tmp-state-0123456789abcdef"] {
            assert!(state_preparation_name(OsStr::new(name)));
            assert_eq!(
                check_output_name(&Path::new("directory").join(name))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for name in [
            ".hive-tmp-ABC123",
            ".comb-tmp-ABC123",
            ".tmpABC123",
            ".hive-tmp-state-ABCDE",
            ".hive-tmp-state-ABCDEFG",
            ".hive-tmp-state-0123456789abcdeF",
            ".hive-tmp-state-ABC_12",
            ".hive-tmp-state-ABC123.json",
        ] {
            assert!(!state_preparation_name(OsStr::new(name)), "{name}");
            check_output_name(Path::new(name)).unwrap();
        }
        check_output_name(Path::new(".hive-tmp-state-ABC123/ordinary.json")).unwrap();
    }
}
