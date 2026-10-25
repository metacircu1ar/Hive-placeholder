//! Private Claude shell entry points. These modes are reached through immutable
//! copies of this executable, never through agent-controlled shell aliases.
use crate::process;
use serde::{Deserialize, Serialize};
use std::{env, error::Error as StdError, ffi::OsStr, io, iter, path::Path, process::exit};
const SUPERVISION_FAILED: &str = "Shell supervision failed.";

mod diagnostics;
pub(crate) mod guard;
mod helper;

pub(crate) const TOKEN_PREFIX: &str = "HIVE_SHELL_GUARD:";
pub(crate) const SOCKET_ENV: &str = "HIVE_SHELL_GUARD_SOCKET";
#[cfg(target_os = "linux")]
pub(crate) const SOCKET_FD_ENV: &str = "HIVE_SHELL_GUARD_FD";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    protocol: hive_input::compatibility::Exact<{ crate::compatibility::SHELL.current }>,
    request: T,
}
impl<T> Envelope<T> {
    fn new(request: T) -> Self {
        Self {
            protocol: hive_input::compatibility::Exact,
            request,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Register {
        token: String,
        pid: u32,
        socket_fd: i32,
    },
    Spawned {
        pid: u32,
    },
    Finished {
        diagnostics: diagnostics::Diagnostics,
    },
    Failed {
        diagnostics: diagnostics::Diagnostics,
    },
}

/// Shell modes terminate with their shell's status.
pub(crate) fn dispatch() {
    let Some(program) = env::args_os().next() else {
        return;
    };
    let name = Path::new(&program).file_name().and_then(OsStr::to_str);
    let result = match name {
        Some("hive-bash") => helper::shell(),
        Some("hive-shell") => helper::supervise(),
        _ => return,
    };
    match result {
        Ok(status) => exit(status),
        Err(error) => {
            eprintln!("Hive shell: {error}");
            for cause in
                iter::successors(StdError::source(&error), |cause| StdError::source(*cause))
            {
                if let Some(io) = cause.downcast_ref::<io::Error>() {
                    eprintln!("OS error: {:?}", io.raw_os_error());
                }
            }
            exit(process::EXEC_FAILED_EXIT);
        }
    }
}

#[cfg(test)]
#[test]
fn compatibility_shell_envelope_contract_is_strict() {
    let envelope = Envelope::new(Request::Spawned { pid: 123 });
    crate::compatibility::contract("shell-envelope.json", &envelope);
    let original = serde_json::to_value(envelope).unwrap();
    for protocol in [
        None,
        Some(serde_json::json!(0)),
        Some(serde_json::json!(2)),
        Some(serde_json::json!(null)),
    ] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove("protocol");
        if let Some(protocol) = protocol {
            value["protocol"] = protocol;
        }
        assert!(serde_json::from_value::<Envelope<Request>>(value).is_err());
    }
}
