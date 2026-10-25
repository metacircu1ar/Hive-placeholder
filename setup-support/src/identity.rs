//! Build-time provenance, distinct from executable identity and format admission.

/// Checkout HEAD when compiled, not proof of an unmodified source tree. Source
/// archives without a Git entry report None rather than inventing an identity.
pub const COMMIT: Option<&str> = if env!("HIVE_SOURCE_COMMIT").is_empty() {
    None
} else {
    Some(env!("HIVE_SOURCE_COMMIT"))
};

#[cfg(test)]
mod source;
