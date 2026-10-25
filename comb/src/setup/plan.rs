use super::{Options, Path, PathBuf, Result, bridge, invalid, paths, provision};
use serde::Serialize;
#[derive(Clone, Serialize)]
pub struct Plan {
    pub root: PathBuf,
    pub socket: PathBuf,
    pub http: Option<crate::embedding::HttpConfig>,
    /// Select text search only: an interrupted fresh setup without an endpoint.
    pub text_only: bool,
    pub initialize: bool,
    pub recover_activation: bool,
    pub bridge_file: Option<PathBuf>,
    pub reviewer_roles: Vec<String>,
}
impl Plan {
    pub fn inspect(root: &Path, socket: Option<&Path>, options: &Options) -> Result<Self> {
        Self::inspect_prepared(root, socket, &options.clone().prepare()?)
    }

    pub(crate) fn inspect_prepared(
        root: &Path,
        socket: Option<&Path>,
        input: &super::Prepared,
    ) -> Result<Self> {
        let options = input.options();
        let reviewer_roles = reviewer_roles(options)?;
        let root = paths::absolute(root)?;
        let initialize = !crate::disk::exists(&root.join(".comb"))?;
        if !initialize {
            crate::disk::Disk::human_authority(&root)?;
        }
        let intent = super::intent::read(&root)?;
        let pending = if initialize {
            None
        } else {
            provision::pending_settings(&root)?
        };
        let saved = if initialize {
            None
        } else {
            pending.clone().or(provision::settings(&root)?)
        };
        let socket = socket_path(&root, socket, saved.as_ref(), intent.as_ref())?;
        if saved.as_ref().is_some_and(|s| s.socket != socket) {
            return Err(invalid(
                "Existing serve.json uses another socket; omit --socket to preserve it.",
            ));
        }
        let http = http_choice(input, saved.is_some(), intent.as_ref())?;
        if !initialize && saved.is_none() && intent.is_none() && http.is_none() {
            return Err(invalid(
                "Existing root has no managed serving settings. Select text search with \
                    comb model use none, or pass --embedding-config.",
            ));
        }
        let text_only = http.is_none() && saved.is_none() && intent.is_some();
        let bridge_file = if options.for_hive {
            Some(
                options
                    .bridge_file
                    .as_deref()
                    .map_or_else(paths::bridge, paths::absolute)?,
            )
        } else {
            None
        };
        let plan = Self {
            root,
            socket,
            http,
            text_only,
            initialize,
            recover_activation: pending.is_some() || intent.is_some(),
            bridge_file,
            reviewer_roles,
        };
        if let Some(intent) = &intent {
            intent.check(&plan)?;
        }
        super::validate::paths(&plan)?;
        bridge::inspect(&plan)?;
        Ok(plan)
    }
}

fn reviewer_roles(options: &Options) -> Result<Vec<String>> {
    let mut roles = options.reviewer_role.clone();
    crate::service::auth::Grant::Bridge {
        name: "hive-setup".into(),
        reviewer_roles: roles.clone(),
    }
    .validate()?;
    roles.sort();
    Ok(roles)
}

fn http_choice(
    input: &super::Prepared,
    saved: bool,
    intent: Option<&super::intent::Intent>,
) -> Result<Option<crate::embedding::HttpConfig>> {
    let http = match &input.http {
        Some(config) => Some(config.clone()),
        None if !saved => intent.and_then(|i| i.http.clone()),
        None => None,
    };
    if let Some(http) = &http {
        crate::embedding::Engine::http_identity(http.clone())?;
    }
    Ok(http)
}

fn socket_path(
    root: &Path,
    socket: Option<&Path>,
    saved: Option<&provision::ServeSettings>,
    intent: Option<&super::intent::Intent>,
) -> Result<PathBuf> {
    let socket = paths::absolute(&socket.map_or_else(
        || {
            saved
                .as_ref()
                .map(|s| s.socket.clone())
                .or_else(|| intent.as_ref().map(|i| i.socket.clone()))
                .unwrap_or_else(|| root.join(".comb/service.sock"))
        },
        Path::to_path_buf,
    ))?;
    if socket.as_os_str().as_encoded_bytes().len() > 103 {
        return Err(invalid(
            "Comb socket path exceeds 103 bytes; pass --socket with a shorter absolute path.",
        ));
    }
    Ok(socket)
}
