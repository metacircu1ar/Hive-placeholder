use super::*;
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::symlink,
        io::{AsRawFd, FromRawFd},
    },
    path::PathBuf,
};

#[test]
fn server_socket_path_errors_are_inputs_but_native_io_failures_remain_operational() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("../missing/socket");
    let selected = Context::new("setup", "--socket");
    let invalid = selected
        .socket_output(&path, comb::setup::check_socket_output(&path))
        .unwrap_err()
        .render();
    assert_eq!(invalid.exit_code, 2);
    assert!(invalid.stdout.contains("\"--socket\""));
    let error = selected
        .socket_output(
            &path,
            Err(Error::from(std::io::Error::from_raw_os_error(libc::EIO))),
        )
        .unwrap_err();
    assert_eq!(error.code(), "io_error");
    assert_eq!(error.render().exit_code, 1);
}

fn http(tokenizer: &str) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"backend":"http","model":"tiny","revision":"pin",
        "url":"http://127.0.0.1:8081/v1/embeddings","dimensions":3,
        "tokenizer":tokenizer,"max_tokens":32}),
    )
    .unwrap()
}
fn tokenizer(config: Config) -> PathBuf {
    let Config::Http(config) = config;
    config.tokenizer.unwrap()
}
#[test]
fn paths_use_selected_config_parent_and_do_not_follow_its_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    let selected = dir.path().join("selected");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&selected).unwrap();
    let file = original.join("config.json");
    let link = selected.join("config.json");
    fs::write(&file, http("assets/tokenizer.json")).unwrap();
    symlink(&file, &link).unwrap();
    let input = Context::new("embed", "--config");
    assert_eq!(
        tokenizer(input.embedding(&file).unwrap()),
        original.join("assets/tokenizer.json")
    );
    assert_eq!(
        tokenizer(input.embedding(&link).unwrap()),
        selected.join("assets/tokenizer.json")
    );
    let value = serde_json::json!({"backend":"http","model":"tiny","revision":"pin","url":"http://127.0.0.1:8081/v1/embeddings","dimensions":3,"tokenizer":"vocab/tokenizer.json","max_tokens":32});
    fs::write(&file, value.to_string()).unwrap();
    let Config::Http(config) = input.embedding(&link).unwrap();
    assert_eq!(
        config.tokenizer.unwrap(),
        selected.join("vocab/tokenizer.json")
    );
    fs::write(&file, http("")).unwrap();
    assert_eq!(input.embedding(&file).unwrap_err().code(), "invalid_file");
}
#[test]
fn pipe_configs_require_absolute_assets_from_the_opened_descriptor() {
    for asset in ["models/tiny", "/absolute/tiny"] {
        let mut descriptors = [-1; 2];
        // SAFETY: pipe receives storage for two descriptors; ownership transfers once.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: both descriptors were just created by pipe and are uniquely owned.
        let reader = unsafe { fs::File::from_raw_fd(descriptors[0]) };
        // SAFETY: the write descriptor was created by pipe and remains uniquely owned.
        let mut writer = unsafe { fs::File::from_raw_fd(descriptors[1]) };
        writer.write_all(&http(asset)).unwrap();
        drop(writer);
        let path = PathBuf::from(format!("/dev/fd/{}", reader.as_raw_fd()));
        let result = Context::new("embed", "--config").embedding(&path);
        if asset.starts_with('/') {
            assert_eq!(tokenizer(result.unwrap()), PathBuf::from(asset));
        } else {
            let output = result.unwrap_err().render();
            assert_eq!(output.exit_code, 2);
            assert!(output.stdout.contains("absolute asset paths"));
        }
    }
}
#[test]
fn named_fifo_config_has_no_relative_asset_base() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.fifo");
    let native = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: native is a live NUL-terminated path and the mode is private.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let input = Context::new("embed", "--config");
    for asset in ["relative", "/absolute/tiny"] {
        let path_copy = path.clone();
        let writer = std::thread::spawn(move || {
            fs::OpenOptions::new()
                .write(true)
                .open(path_copy)
                .unwrap()
                .write_all(&http(asset))
                .unwrap();
        });
        let result = input.embedding(&path);
        writer.join().unwrap();
        if asset.starts_with('/') {
            assert_eq!(tokenizer(result.unwrap()), PathBuf::from(asset));
        } else {
            assert!(
                result
                    .unwrap_err()
                    .render()
                    .stdout
                    .contains("absolute asset paths")
            );
        }
    }
}
