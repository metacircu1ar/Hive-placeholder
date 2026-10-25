use super::*;
use std::{
    io::{BufRead, Write},
    os::unix::net::UnixListener,
};

provider_matrix!(
    typed_comb_input_survives_hosted_provider_replies,
    typed_input_case
);

fn typed_input_case(provider: Provider) {
    let private = tempfile::tempdir_in("/tmp").unwrap();
    let socket = private.path().join("errors.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let credential = private.path().join("client.json");
    Connection {
        format: hive_input::compatibility::Exact,
        socket,
        credential: ::comb::service::auth::Credential {
            comb_id: ::comb::values::CombId::generate(),
            id: ::comb::values::CredentialId::generate(),
            secret: "fixture".into(),
        },
    }
    .write(&credential)
    .unwrap();
    let mut f = Harness::builder(provider).build();
    f.edit_config(|config| config["comb"] = json!({"credential_file":credential}));
    f.start();
    let id = spawn(
        &f,
        provider,
        false,
        json!([{"name":"comb_search","arguments":{}}]),
    );
    let reply = results(&f, &id, 1);
    assert_eq!(reply[0]["error"]["code"], "missing_argument");
    assert_eq!(reply[0]["error"]["field"], "/arguments/query");
    assert!(reply[0]["error"]["examples"].is_array());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    for (name, arguments, field) in [
        (
            "comb_capture",
            json!({"text":"private-body","request_id":null}),
            "/arguments/request_id",
        ),
        (
            "comb_put",
            json!({"slug":"../private-slug","markdown":"private-body"}),
            "/arguments/slug",
        ),
    ] {
        f.ok(json!({"op":"send","target":id,"body":body(json!([{"name":name,"arguments":arguments}]))}));
        let turn = if name == "comb_capture" { 2 } else { 3 };
        let reply = results(&f, &id, turn);
        assert_eq!(reply[0]["error"]["field"], field);
        assert_eq!(reply[0]["error"]["code"], "invalid_value");
        assert!(!reply[0]["error"].to_string().contains("private-"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    let server = std::thread::spawn(move || reply_with_input(listener));
    f.ok(json!({"op":"send","target":id,"body":body(json!([{"name":"comb_get","arguments":{"slug":"concepts/page"}}]))}));
    let reply = results(&f, &id, 4);
    let request = server.join().unwrap();
    assert_eq!(request["claim"]["id"], id);
    assert_eq!(request["operation"]["slug"], "concepts/page");
    let error = &reply[0]["error"];
    assert_eq!(error["code"], "invalid_value");
    assert_eq!(error["field"], "/operation/slug");
    assert_eq!(error["expected"], "a page slug");
    assert_eq!(error["file_committed"], true);
    assert_eq!(error["request_id"], "server-receipt");
    assert_eq!(
        error["examples"],
        json!(["comb_get {\"slug\":\"concepts/page\"}"])
    );
    assert!(!error.to_string().contains("private-marker"));
    f.shutdown();
}

fn reply_with_input(listener: UnixListener) -> Value {
    let socket = support::wait::wait_until(
        Duration::from_secs(15),
        Duration::from_millis(10),
        "hosted Comb socket",
        || listener.accept().ok().map(|(socket, _)| socket),
    );
    socket.set_nonblocking(false).unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut socket = std::io::BufReader::new(socket);
    let mut frame = String::new();
    socket.read_line(&mut frame).unwrap();
    let request = serde_json::from_str(&frame).unwrap();
    let input = hive_input::InputError::new(
        hive_input::Code::InvalidValue,
        "Invalid retrieval input.",
        "/operation/slug",
        "a page slug",
        vec!["comb_get {\"slug\":\"concepts/page\"}".into()],
    )
    .unwrap()
    .with_public_received("private-marker");
    let mut error = ::comb::Error::from(input).committed();
    error.request_id = Some("server-receipt".into());
    let reply = comb_protocol::service::api::Reply::Error {
        protocol: hive_input::compatibility::Exact,
        error,
    };
    let mut bytes = serde_json::to_vec(&reply).unwrap();
    bytes.push(b'\n');
    socket.get_mut().write_all(&bytes).unwrap();
    request
}
