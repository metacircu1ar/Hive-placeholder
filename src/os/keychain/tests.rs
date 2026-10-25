//! No user Keychain calls: SDK query construction runs against native substitutes.
#[test]
fn native_queries_use_exact_attributes_and_persistent_item_list_without_password_data() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("keychain-query-fixture");
    let output = crate::os::output(
        std::process::Command::new("/usr/bin/clang")
            .args([
                "-O3",
                "-Werror",
                "-framework",
                "Security",
                "-framework",
                "Foundation",
                "-framework",
                "LocalAuthentication",
                "-framework",
                "CoreFoundation",
            ])
            .arg(root.join("tests/fixtures/keychain_query.m"))
            .arg("-o")
            .arg(&binary),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        crate::os::spawn(&mut std::process::Command::new(binary))
            .unwrap()
            .wait()
            .unwrap()
            .success()
    );
}
