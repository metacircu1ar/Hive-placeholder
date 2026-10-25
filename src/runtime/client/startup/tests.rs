use super::*;
use std::{error::Error as _, io, os::unix::net::UnixListener};

#[test]
fn daemon_start_absence_is_only_local_not_found_or_refused() {
    for kind in [io::ErrorKind::NotFound, io::ErrorKind::ConnectionRefused] {
        assert!(absent(&kind.into()));
    }
    for kind in [
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::TimedOut,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::InvalidInput,
    ] {
        assert!(!absent(&kind.into()));
    }
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::create_dir(tmp.path().join("runtime")).unwrap();
    let rt = crate::calls::executor().unwrap();
    let deadline = || Instant::now() + std::time::Duration::from_secs(1);
    assert!(
        rt.block_on(Client::new(tmp.path()).startup(deadline()))
            .unwrap()
            .is_none()
    );
    drop(UnixListener::bind(socket_path(tmp.path())).unwrap());
    assert!(
        rt.block_on(Client::new(tmp.path()).startup(deadline()))
            .unwrap()
            .is_none()
    );
    let error = rt
        .block_on(Client::new(&socket_path(tmp.path())).startup(deadline()))
        .unwrap_err();
    assert_eq!(error.code(), "daemon_unavailable");
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .is_some()
    );
}

#[test]
fn daemon_start_connected_failures_never_become_absence_and_deadline_is_bounded() {
    let rt = crate::calls::executor().unwrap();
    for response in [
        Some("invalid"),
        Some(r#"{"error":{"code":"daemon_unavailable","message":"busy"}}"#),
        None,
    ] {
        let tmp = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::create_dir(tmp.path().join("runtime")).unwrap();
        let started = Instant::now();
        let result = rt.block_on(async {
            let listener = tokio::net::UnixListener::bind(socket_path(tmp.path())).unwrap();
            let server = async {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = tokio::io::BufReader::new(stream);
                let mut request = String::new();
                reader.read_line(&mut request).await.unwrap();
                if let Some(text) = response {
                    reader
                        .get_mut()
                        .write_all(format!("{text}\n").as_bytes())
                        .await
                        .unwrap();
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::pin!(server);
            let probe = Client::new(tmp.path());
            let exchange = probe.startup(started + std::time::Duration::from_millis(50));
            tokio::pin!(exchange);
            tokio::select! {
                result = &mut exchange => result,
                () = &mut server => exchange.await,
            }
        });
        match response {
            Some("invalid") => assert_eq!(result.unwrap_err().code(), "daemon_protocol_error"),
            Some(_) => assert!(result.unwrap().unwrap().get("error").is_some()),
            None => {
                let error = result.unwrap_err();
                assert_eq!(error.code(), "daemon_start_timeout");
                assert!(error.source().is_some());
                assert!(started.elapsed() < std::time::Duration::from_secs(2));
            }
        }
    }
}
