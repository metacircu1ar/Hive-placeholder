//! Exactly owned user services. Readiness is supplied by each product's health API.
mod held;
mod lifecycle;
mod manager;
mod refresh;
mod registration;
mod render;
use crate::{Result, files, invalid};
pub use held::Held;
pub use lifecycle::{Controller, Health, RestartFailure, StartDecision, Status};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Install a user login service; run `service start` to start it now.
    Install {
        /// Private mode-0600 KEY=literal-value file. Values never enter the unit.
        #[arg(long)]
        environment_file: Option<PathBuf>,
        /// Stable absolute executable path (defaults to the invoked executable).
        #[arg(long)]
        program: Option<PathBuf>,
        /// Toolchain PATH; defaults to absolute existing directories from this shell.
        #[arg(long)]
        service_path: Option<String>,
    },
    /// Start the installed user service and wait for readiness.
    Start,
    /// Stop the owned user service.
    Stop,
    /// Inspect the installed user service and its readiness.
    Status,
    /// Remove the owned user service while retaining user data.
    Uninstall,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Launchd,
    Systemd,
}
impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Launchd
        } else {
            Self::Systemd
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub product: String,
    pub paths: Vec<PathBuf>,
}
impl Scope {
    pub fn new(product: &str, paths: &[&Path]) -> Result<Self> {
        if !matches!(product, "hive" | "comb") {
            return Err(invalid("Unknown service product."));
        }
        if paths.len() != if product == "hive" { 2 } else { 1 } {
            return Err(invalid("Wrong service scope paths."));
        }
        Ok(Self {
            product: product.into(),
            paths: paths
                .iter()
                .map(|p| files::location(p))
                .collect::<Result<_>>()?,
        })
    }
    pub fn label(&self) -> Result<String> {
        let hash = Sha256::digest(serde_json::to_vec(self).map_err(std::io::Error::other)?);
        Ok(format!(
            "io.hive.{}.{:.16}",
            self.product,
            format!("{hash:x}")
        ))
    }
    pub fn foreground_command(&self) -> Result<String> {
        let quoted = self
            .paths
            .iter()
            .map(|p| path_text(p).map(|s| format!("'{}'", s.replace('\'', "'\\''"))))
            .collect::<Result<Vec<_>>>()?;
        match (self.product.as_str(), quoted.as_slice()) {
            ("hive", [config, state]) => Ok(format!(
                "hivectl --config {config} --state-dir {state} daemon run"
            )),
            ("comb", [root]) => Ok(format!("comb --root {root} serve")),
            _ => Err(invalid("Invalid service scope.")),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub home: PathBuf,
    pub config: PathBuf,
    pub state: PathBuf,
    pub platform: Platform,
}
impl Layout {
    pub fn discover() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| invalid("HOME is required."))?;
        if !home.is_absolute() {
            return Err(invalid("HOME must be absolute."));
        }
        let config = xdg("XDG_CONFIG_HOME", &home.join(".config"))?;
        let state = xdg("XDG_STATE_HOME", &home.join(".local/state"))?;
        Ok(Self {
            home: home.canonicalize()?,
            config,
            state,
            platform: Platform::current(),
        })
    }
    pub fn descriptor(&self, scope: &Scope) -> Result<PathBuf> {
        Ok(self
            .config
            .join("hive/services")
            .join(format!("{}.json", scope.label()?)))
    }
    pub fn unit(&self, scope: &Scope) -> Result<PathBuf> {
        let label = scope.label()?;
        Ok(match self.platform {
            Platform::Launchd => self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
            Platform::Systemd => self
                .config
                .join("systemd/user")
                .join(format!("{label}.service")),
        })
    }
    pub fn log(&self, scope: &Scope) -> Result<PathBuf> {
        Ok(self
            .state
            .join("hive/logs")
            .join(format!("{}.log", scope.label()?)))
    }
}
fn xdg(name: &str, default: &Path) -> Result<PathBuf> {
    let path = std::env::var_os(name).map_or_else(|| default.to_owned(), PathBuf::from);
    if !path.is_absolute() {
        return Err(invalid(format!("{name} must be absolute.")));
    }
    files::location(&path)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub format: hive_input::compatibility::Exact<{ crate::compatibility::SERVICE.current }>,
    pub scope: Scope,
    pub platform: Platform,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment_file: Option<PathBuf>,
    pub required_environment: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub descriptor: PathBuf,
    pub unit: PathBuf,
    pub log: PathBuf,
    pub unit_contents: String,
}
impl Spec {
    pub fn new(
        layout: &Layout,
        scope: Scope,
        program: &Path,
        arguments: Vec<String>,
        environment_file: Option<&Path>,
        required_environment: Vec<String>,
    ) -> Result<Self> {
        let environment = BTreeMap::from([
            ("HOME".into(), path_text(&layout.home)?),
            ("XDG_CONFIG_HOME".into(), path_text(&layout.config)?),
            ("XDG_STATE_HOME".into(), path_text(&layout.state)?),
            ("PATH".into(), crate::environment::service_path(None)?),
        ]);
        let mut spec = Self {
            format: hive_input::compatibility::Exact,
            platform: layout.platform,
            program: files::executable(program)?,
            arguments,
            environment_file: environment_file.map(files::absolute).transpose()?,
            required_environment,
            environment,
            descriptor: layout.descriptor(&scope)?,
            unit: layout.unit(&scope)?,
            log: layout.log(&scope)?,
            scope,
            unit_contents: String::new(),
        };
        spec.unit_contents = String::from_utf8(spec.render()?).map_err(std::io::Error::other)?;
        Ok(spec)
    }
    pub fn render(&self) -> Result<Vec<u8>> {
        render::unit(self)
    }
    pub fn rendered(&self) -> Result<Self> {
        let mut desired = self.clone();
        desired.unit_contents = String::from_utf8(self.render()?).map_err(std::io::Error::other)?;
        Ok(desired)
    }
    pub fn missing_environment(&self) -> Result<Vec<String>> {
        crate::environment::missing(self.environment_file.as_deref(), &self.required_environment)
    }
    pub fn check_environment(&self) -> Result<()> {
        let missing = self.missing_environment()?;
        if !missing.is_empty() {
            return Err(invalid(format!(
                "Service environment file is missing required names: {}. Supply \
                    --environment-file.",
                missing.join(", ")
            )));
        }
        Ok(())
    }
    pub fn verify(&self, layout: &Layout, scope: &Scope) -> Result<()> {
        self.verify_record(layout, scope)?;
        if files::read(&self.unit)?.as_deref() != Some(self.unit_contents.as_bytes()) {
            return Err(invalid(
                "Service unit was edited; refusing to control or remove it.",
            ));
        }
        Ok(())
    }
    pub(super) fn verify_record(&self, layout: &Layout, scope: &Scope) -> Result<()> {
        self.verify_layout(layout, scope)?;
        if files::read(&self.descriptor)?.as_deref() != Some(files::encode(self)?.as_slice()) {
            return Err(invalid(
                "Service descriptor or unit was edited; refusing to control or remove it.",
            ));
        }
        Ok(())
    }
    /// Validate frozen ownership paths without requiring the removed unit to exist.
    pub fn verify_layout(&self, layout: &Layout, scope: &Scope) -> Result<()> {
        if self.scope != *scope
            || self.platform != layout.platform
            || self.descriptor != layout.descriptor(scope)?
            || self.unit != layout.unit(scope)?
            || self.log != layout.log(scope)?
        {
            return Err(invalid(
                "Service descriptor does not match this scope and user layout.",
            ));
        }
        Ok(())
    }
}
pub fn load(layout: &Layout, scope: &Scope) -> Result<Option<Spec>> {
    let path = layout.descriptor(scope)?;
    let spec: Option<Spec> = files::json(&path).map_err(|cause| {
        crate::context(
            "Cannot read current service record. For an unsupported development format, \
        uninstall with the currently installed hive-install or service uninstall \
        command before installing new binaries",
            cause,
        )
    })?;
    if let Some(spec) = &spec {
        spec.verify(layout, scope)?;
    }
    Ok(spec)
}
pub fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("Service paths must be UTF-8."))
}
