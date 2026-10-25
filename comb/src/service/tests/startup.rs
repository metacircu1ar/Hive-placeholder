use super::*;
use std::{fs, time::Instant};

fn config(revision: &str) -> embedding::Config {
    serde_json::from_value(json!({"backend":"http","model":"tiny","revision":revision,"dimensions":3,"url":"http://127.0.0.1:9/v1/embeddings"})).unwrap()
}
#[test]
fn invalid_or_occupied_startup_preserves_unsynced_pages_and_active_space() {
    let dir = crate::test_support::private_directory().unwrap();
    let mut comb = Comb::init(dir.path()).unwrap();
    let old = Engine::load(config("old")).unwrap();
    comb.activate_embeddings(old.space(), false).unwrap();
    let active = comb.index().unwrap().active_space().unwrap();
    drop(comb);
    fs::write(
        dir.path().join("external.md"),
        "---\ntitle: Not synced\n---\nExternal change.\n",
    )
    .unwrap();
    let path = dir.path().join("occupied.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    for (cache, socket, expected) in [
        (0, None, crate::ErrorCode::InvalidCacheBudget),
        (
            512,
            Some(dir.path().join("missing/socket")),
            crate::ErrorCode::IoError,
        ),
        (512, Some(path.clone()), crate::ErrorCode::CombBusy),
    ] {
        let error = Server::prepare(
            dir.path(),
            Options {
                socket,
                embeddings: Some(config("new")),
                replace_embeddings: true,
                vector_cache_mib: Some(cache),
                ..Options::default()
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.code, expected);
        let reopened = Comb::open(dir.path()).unwrap();
        assert_eq!(reopened.index().unwrap().stats().unwrap().pages, 0);
        assert_eq!(reopened.index().unwrap().active_space().unwrap(), active);
    }
    drop(listener);
    assert!(path.exists());
}
#[test]
fn full_socket_backlog_is_bounded_and_never_removed() {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, bind, connect, listen, socket};
    let dir = crate::test_support::private_directory().unwrap();
    drop(Comb::init(dir.path()).unwrap());
    let path = dir.path().join("full.sock");
    let listener = socket(AddressFamily::UNIX, SocketType::STREAM, None).unwrap();
    let address = SocketAddrUnix::new(&path).unwrap();
    bind(&listener, &address).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    listen(&listener, 1).unwrap();
    let mut clients = Vec::new();
    let mut full = false;
    for _ in 0..32 {
        let fd = socket(AddressFamily::UNIX, SocketType::STREAM, None).unwrap();
        let stream = std::os::unix::net::UnixStream::from(fd);
        stream.set_nonblocking(true).unwrap();
        let result = connect(&stream, &address);
        eprintln!("backlog fixture connect {}: {result:?}", clients.len());
        clients.push(stream);
        if result.is_err() {
            full = true;
            break;
        }
    }
    assert!(full, "fixture must saturate the backlog");
    let start = Instant::now();
    let error = Server::prepare(
        dir.path(),
        Options {
            socket: Some(path.clone()),
            ..Options::default()
        },
    )
    .err()
    .unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(error.code, crate::ErrorCode::CombBusy);
    assert!(path.exists());
}

#[test]
fn relative_socket_filename_uses_the_working_directory() {
    assert_eq!(
        super::socket_path(Path::new("unused"), Some(Path::new("service.sock"))).unwrap(),
        std::env::current_dir().unwrap().join("service.sock")
    );
}
