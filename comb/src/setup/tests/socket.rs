use super::*;

#[test]
fn socket_privacy_setup_creates_missing_private_directories_and_reuses_them() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path().join("root");
    let socket = temp.path().join("private/nested/s.sock");
    let options = Options {
        no_services: true,
        ..Options::default()
    };
    check_socket_output(&socket).unwrap();
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    run_offline(&root, Some(&socket), options.clone()).unwrap();
    for directory in [
        temp.path().join("private"),
        temp.path().join("private/nested"),
    ] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let before = snapshot(temp.path());
    run_offline(&root, Some(&socket), options).unwrap();
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn socket_privacy_setup_refuses_public_parent_and_unsafe_ancestry_without_provisioning() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path().join("root");
    let public = temp.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    let options = Options {
        no_services: true,
        ..Options::default()
    };
    assert!(check_socket_output(&public.join("s.sock")).is_err());
    assert!(run_offline(&root, Some(&public.join("s.sock")), options.clone()).is_err());
    fs::set_permissions(&public, fs::Permissions::from_mode(0o777)).unwrap();
    let missing = public.join("missing/s.sock");
    assert!(check_socket_output(&missing).is_err());
    assert!(run_offline(&root, Some(&missing), options).is_err());
    assert!(!root.exists());
    assert_eq!(fs::read_dir(&public).unwrap().count(), 0);
    assert_eq!(
        fs::metadata(&public).unwrap().permissions().mode() & 0o777,
        0o777
    );
}
