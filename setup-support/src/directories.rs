//! Directory permission policy; regular files and private leaves use stricter checks.
/// An existing directory belonging to this user. Group members are trusted only
/// for non-root user directories; world-write and foreign ownership are refused.
pub fn owned(owner: u32, mode: impl Into<u32>, user: u32) -> bool {
    let mode = mode.into();
    owner == user && mode & if owner == 0 { 0o022 } else { 0o002 } == 0
}

/// Route/lock ancestors may also be root-owned, including root-owned sticky
/// directories such as /tmp. This exception does not apply to private leaves.
pub fn ancestor(owner: u32, mode: impl Into<u32>, user: u32) -> bool {
    let mode = mode.into();
    if owner == 0 {
        mode & 0o022 == 0 || mode & 0o1000 != 0
    } else {
        owned(owner, mode, user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_mode_policy_keeps_root_sticky_exception_scoped_to_ancestors() {
        // Each column is an independent expectation, including root-as-caller.
        for (owner, mode, user, expected_owned, expected_ancestor) in [
            (1000, 0o700_u32, 1000, true, true),
            (1000, 0o755, 1000, true, true),
            (1000, 0o770, 1000, true, true),
            (1000, 0o775, 1000, true, true),
            (1000, 0o2775, 1000, true, true),
            (1000, 0o777, 1000, false, false),
            (1000, 0o1777, 1000, false, false),
            (1001, 0o755, 1000, false, false),
            (1001, 0o775, 1000, false, false),
            (1001, 0o1777, 1000, false, false),
            (0, 0o755, 1000, false, true),
            (0, 0o775, 1000, false, false),
            (0, 0o777, 1000, false, false),
            (0, 0o1777, 1000, false, true),
            (0, 0o755, 0, true, true),
            (0, 0o775, 0, false, false),
            (0, 0o1777, 0, false, true),
            (1000, 0o775, 0, false, false),
        ] {
            assert_eq!(
                owned(owner, mode, user),
                expected_owned,
                "{owner}:{mode:o}:{user}"
            );
            assert_eq!(
                ancestor(owner, mode, user),
                expected_ancestor,
                "{owner}:{mode:o}:{user}"
            );
        }
    }
}
