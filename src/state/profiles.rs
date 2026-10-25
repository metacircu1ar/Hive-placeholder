//! Durable profile bindings and registration index transactions.
use super::{
    Lock, check_path, corrupt, directory, generations::GenerationState, read_json, write_json,
};
use crate::{
    config::{Profile, Provider},
    error::{Error, Result},
    runtime,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::ErrorKind,
    path::{Component, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: Uuid,
    #[serde(deserialize_with = "crate::config::stored_profile")]
    pub profile: Profile,
    pub ready: bool,
    pub directory: PathBuf,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Index {
    pub(super) profiles: BTreeMap<String, Binding>,
    pub(super) generations: BTreeMap<Uuid, super::generations::Generation>,
}

#[derive(Clone)]
pub struct State {
    pub root: PathBuf,
    pub(super) runtime: Option<std::sync::Arc<super::admission::Runtime>>,
}
fn unsupported_format() -> Error {
    Error::unsupported_schema(concat!(
        "Unsupported development state format. Use a new --state-dir and run auth",
        " login again; this directory was not modified.",
    ))
}
fn database_format(result: Result<()>) -> Result<()> {
    result.map_err(|cause| {
        if matches!(cause, Error::NewerFormat { .. }) {
            Error::unsupported_schema(
                "The state marker and database format disagree. Retain this directory unchanged.",
            )
            .with_source(cause)
        } else {
            cause
        }
    })
}
impl State {
    pub fn new(root: PathBuf) -> Result<Self> {
        if !root.is_absolute() {
            return Err(Error::invalid_path("State directory must be absolute."));
        }
        Ok(Self {
            root,
            runtime: None,
        })
    }
    /// Parse the complete index structure before acquiring locks or writing state.
    pub fn check_format(&self) -> Result<()> {
        if let Some(runtime) = &self.runtime {
            return runtime.check(self);
        }
        // Reject unsupported file shapes without side effects, then exclude
        // migration before native SQLite can touch WAL shared-memory state.
        self.load_index(false, super::Replaced::Reopen)?;
        let admission = self.admission()?;
        self.check_format_owned(&admission)
    }
    pub(super) fn check_format_owned(&self, admission: &super::admission::Shared) -> Result<()> {
        if let Some(runtime) = &self.runtime {
            return runtime.check(self);
        }
        admission.check().map_err(Error::state_io_from)?;
        // Format checks run before the index lock, beside registrations.
        self.load_index(true, super::Replaced::Reopen)?;
        database_format(runtime::check_existing_identity(
            &self.root.join("runtime/runtime.sqlite"),
        ))
    }
    fn load_index(&self, check_database: bool, replaced: super::Replaced) -> Result<Option<Index>> {
        if !super::recovery::exists(&self.root)? {
            return Ok(None);
        }
        check_path(&self.root, true)?;
        super::format::check(&self.root)?;
        let path = self.root.join("index.json");
        if !super::recovery::exists(&path)? {
            if check_database {
                database_format(runtime::check_existing_format(
                    &self.root.join("runtime/runtime.sqlite"),
                ))?;
            }
            return Ok(None);
        }
        let bytes = super::read_json_bytes(&path, replaced)?;
        super::parse_record(&bytes)
            .map(Some)
            .map_err(|error| unsupported_format().with_source(error))
    }
    pub(super) fn read_index(&self) -> Result<Index> {
        self.validate_index(self.load_index(true, super::Replaced::Refuse)?)
    }
    /// Migration inspects SQLite only through a private snapshot. Even a
    /// read-only connection can create or update the original WAL's SHM file.
    pub(super) fn migration_index(&self) -> Result<Index> {
        self.validate_index(self.load_index(false, super::Replaced::Refuse)?)
    }
    fn validate_index(&self, index: Option<Index>) -> Result<Index> {
        if matches!(
            self.root.symlink_metadata(),
            Err(e) if e.kind() == ErrorKind::NotFound
        ) {
            return Ok(Index::default());
        }
        check_path(&self.root, true)?;
        let profiles = self.root.join("profiles");
        let Some(index) = index else {
            if profiles.try_exists().map_err(Error::state_io_from)? {
                check_path(&profiles, true)?;
                if fs::read_dir(&profiles)
                    .map_err(Error::state_io_from)?
                    .next()
                    .is_some()
                {
                    return Err(corrupt());
                }
            }
            return Ok(Index::default());
        };
        let ids = index.validate_records()?;
        // Orphaned registration directories are not silently adopted or discarded.
        if profiles.try_exists().map_err(Error::state_io_from)? {
            check_path(&profiles, true)?;
            for entry in fs::read_dir(&profiles).map_err(Error::state_io_from)? {
                let name = entry.map_err(Error::state_io_from)?.file_name();
                if name.to_str().is_some_and(|s| s.starts_with('.')) {
                    continue;
                }
                if !name
                    .to_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .is_some_and(|id| ids.contains(&id))
                {
                    return Err(corrupt());
                }
            }
        }
        Ok(index)
    }
    pub(super) fn binding_location(&self, binding: &Binding) -> Result<PathBuf> {
        let expected = self.root.join("profiles").join(binding.id.to_string());
        {
            let dir = &binding.directory;
            // Preserve the original path bytes (Claude hashes them), while
            // rejecting a moved root or a binding redirected outside this root.
            if !dir.is_absolute() || dir.components().any(|c| matches!(c, Component::ParentDir)) {
                return Err(corrupt());
            }
            match (dir.canonicalize(), expected.canonicalize()) {
                (Ok(actual), Ok(expected)) if actual == expected => {}
                (Err(actual), Err(expected_error))
                    if !binding.ready
                        && dir == &expected
                        && actual.kind() == ErrorKind::NotFound
                        && expected_error.kind() == ErrorKind::NotFound => {}
                (Err(error), _) | (_, Err(error)) => {
                    return Err(Error::state_io_from(error));
                }
                _ => return Err(corrupt()),
            }
        }
        Ok(expected)
    }
    pub(super) fn validate_binding(&self, binding: &Binding) -> Result<()> {
        let expected = self.binding_location(binding)?;
        if super::recovery::exists(&expected)? {
            super::recovery::layout::profile(&expected)?;
        }
        if !binding.ready {
            // An index reservation is durable before any directory is created.
            for path in [
                self.root.join("profiles"),
                binding.directory.clone(),
                binding.directory.join("provider"),
                binding.directory.join("work"),
            ] {
                if super::recovery::exists(&path)? {
                    check_path(&path, true)?;
                }
            }
            let marker = binding.directory.join("identity.json");
            if super::recovery::exists(&marker)? {
                let identity: Binding = read_json(&marker)?;
                if !identity.same_identity(binding) {
                    return Err(corrupt());
                }
            }
            return Ok(());
        }
        check_path(&self.root.join("profiles"), true)?;
        let dir = binding.directory.clone();
        check_path(&dir, true)?;
        check_path(&dir.join("provider"), true)?;
        check_path(&dir.join("work"), true)?;
        let identity: Binding = read_json(&dir.join("identity.json"))?;
        if !identity.same_identity(binding) {
            return Err(corrupt());
        }
        Ok(())
    }
    pub fn lookup(&self, name: &str, profile: &Profile) -> Result<Option<Binding>> {
        let _admission = self.admission()?;
        self.check_format()?;
        // Readers must not observe the short directory-before-index registration window.
        let lock_path = self.root.join("index.lock");
        let _index_lock = if super::recovery::exists(&lock_path)? {
            check_path(&self.root, true)?;
            Some(Lock::index(&lock_path)?)
        } else {
            None
        };
        let index = self.read_index()?;
        let binding = Self::lookup_in(&index, name, profile)?;
        if let Some(binding) = &binding {
            self.validate_binding(binding)?;
        }
        Ok(binding)
    }
    fn lookup_in(index: &Index, name: &str, profile: &Profile) -> Result<Option<Binding>> {
        if let Some(binding) = index.profiles.get(name) {
            if &binding.profile != profile {
                return Err(Error::binding_conflict(
                    "Registered profile fields changed. Use a new profile name.",
                ));
            }
            Ok(Some(binding.clone()))
        } else {
            Ok(None)
        }
    }
    pub fn register(&self, name: &str, profile: &Profile) -> Result<Binding> {
        profile.validate()?;
        if profile
            .api_key_file
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(Error::invalid_path(
                "API key file must resolve against the configuration directory.",
            ));
        }
        self.check_format()?;
        let keychain_account = (profile.provider == Provider::Claude)
            .then(super::generations::claude_keychain_account)
            .transpose()?;
        // Only create parents outside the managed root; never chmod existing ancestors.
        let _initialization = self.initialize()?;
        let index_lock = Lock::index(&self.root.join("index.lock"))?;
        self.update_locked_index(&index_lock, |index| {
            let namespaces = self.registration_namespaces(index, profile)?;
            let mut binding = match Self::lookup_in(index, name, profile)? {
                Some(existing) if existing.ready => {
                    self.validate_binding(&existing)?;
                    return Ok(existing);
                }
                Some(existing) => {
                    self.validate_binding(&existing)?;
                    existing
                }
                None => {
                    let (pending, _) = self.pending_binding(profile, &namespaces)?;
                    let id = pending.id;
                    index.generations.insert(
                        id,
                        super::generations::Generation::new(
                            pending.clone(),
                            name,
                            keychain_account,
                        ),
                    );
                    index.profiles.insert(name.into(), pending.clone());
                    index.save()?;
                    pending
                }
            };
            directory(&self.root.join("profiles"))?;
            let dir = binding.directory.clone();
            Self::initialize_binding(&binding)?;
            super::sync_directory(&self.root.join("profiles")).map_err(Error::state_io_from)?;
            binding.ready = true;
            index.profiles.insert(name.into(), binding.clone());
            let generation = index.generation_mut(binding.id)?;
            generation.binding = binding.clone();
            let lock = Lock::acquire_durable(&dir.join("auth.lock"))?;
            generation.credential_lock = Some(super::LockIdentity::from_fd(lock.fd())?);
            index.save()?;
            Ok(binding)
        })
    }
}

#[cfg(test)]
#[test]
fn profile_index_requires_exact_fields_and_generation_state() {
    use crate::config::{Method, Provider};
    let binding = Binding {
        id: Uuid::nil(),
        profile: Profile {
            provider: Provider::Codex,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        },
        ready: false,
        directory: PathBuf::from("/state/profiles/namespace"),
    };
    let generation = super::generations::Generation::new(binding.clone(), "main", None);
    let generation = crate::runtime::api::value(generation);
    super::assert_strict_record::<super::generations::Generation>(&generation, &["state"]);
    super::assert_strict_record::<Binding>(&crate::runtime::api::value(binding), &[]);
    super::assert_strict_record::<Index>(
        &serde_json::json!({"profiles": {}, "generations": {}}),
        &[],
    );
}

impl Index {
    fn validate_records(&self) -> Result<HashSet<Uuid>> {
        let mut ids =
            self.profiles
                .iter()
                .try_fold(HashSet::new(), |mut ids, (name, binding)| {
                    if !ids.insert(binding.id) {
                        return Err(corrupt());
                    }
                    let generation = self.generation(binding.id)?;
                    if !generation.binding.same_identity(binding)
                        || generation.binding.ready != binding.ready
                        || generation.state != GenerationState::Active
                        || generation.profile_name != *name
                    {
                        return Err(corrupt());
                    }
                    Ok(ids)
                })?;

        if self.generations.iter().any(|(id, generation)| {
            *id != generation.binding.id
                || (generation.key_fingerprint.is_none() != (generation.key_revision == 0))
                || (generation.binding.profile.method == crate::config::Method::Subscription
                    && generation.key_fingerprint.is_some())
                || (generation.state == GenerationState::Active && !ids.contains(id))
                || (generation.binding.ready && generation.credential_lock.is_none())
                || (generation.binding.profile.provider == Provider::Claude
                    && generation
                        .keychain_account
                        .as_ref()
                        .is_none_or(String::is_empty))
        }) {
            return Err(corrupt());
        }
        ids.extend(self.generations.keys());
        Ok(ids)
    }
}

impl State {
    pub(super) fn pending_binding(
        &self,
        profile: &Profile,
        namespaces: &HashSet<String>,
    ) -> Result<(Binding, Option<String>)> {
        loop {
            let id = Uuid::new_v4();
            let binding = Binding {
                id,
                profile: profile.clone(),
                ready: false,
                directory: self.root.join("profiles").join(id.to_string()),
            };
            let namespace = (profile.provider == Provider::Claude)
                .then(|| super::generations::claude_namespace(&binding.directory.join("provider")))
                .transpose()?;
            if namespace
                .as_ref()
                .is_none_or(|value| !namespaces.contains(value))
            {
                return Ok((binding, namespace));
            }
        }
    }
    pub(super) fn initialize_binding(binding: &Binding) -> Result<()> {
        let dir = &binding.directory;
        directory(dir)?;
        directory(&dir.join("provider"))?;
        directory(&dir.join("work"))?;
        write_json(&dir.join("identity.json"), binding)?;
        super::sync_directory(dir).map_err(Error::state_io_from)
    }
}

impl State {
    fn registration_namespaces(&self, index: &Index, profile: &Profile) -> Result<HashSet<String>> {
        let mut namespaces = if profile.provider == Provider::Claude {
            self.retired_namespaces()?.into_iter().collect()
        } else {
            HashSet::new()
        };
        namespaces.extend(index.claude_namespaces(None).collect::<Result<Vec<_>>>()?);
        Ok(namespaces)
    }
}

#[cfg(test)]
mod tests;
