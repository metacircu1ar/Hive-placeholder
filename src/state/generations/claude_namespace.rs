//! Predict credential item names; destructive operations verify actual identity.
use super::{Result, corrupt};
use crate::os;
use sha2::{Digest, Sha256};
use std::env;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

/// Compute the config-directory-specific item name; this alone is not deletion authority.
pub fn claude_namespace(path: &Path) -> Result<String> {
    let text = path.to_str().ok_or_else(corrupt)?;
    let hash = Sha256::digest(text.nfc().collect::<String>().as_bytes());
    Ok(format!(
        "Claude Code-credentials-{:02x}{:02x}{:02x}{:02x}",
        hash[0], hash[1], hash[2], hash[3]
    ))
}

/// Native Claude's account key, independently from subscription email.
pub(in crate::state) fn claude_keychain_account() -> Result<String> {
    let user = crate::observability::optional(env::var("USER"), "claude_account_environment")
        .filter(|s| !s.is_empty())
        .map_or_else(os::username, Ok)?;
    Ok(
        if !user.is_empty()
            && user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            user
        } else {
            "claude-code-user".into()
        },
    )
}
