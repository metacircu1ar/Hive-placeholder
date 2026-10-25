//! Server preparation and socket ownership.
use super::{
    Arc, AtomicUsize, CheckedMutex, Comb, Embedder, Engine, Error, Model, Notify, Options, Path,
    PathBuf, Prepared, Registry, Result, Semaphore, Server, State, limits, queries,
};

pub fn socket_path(root: &Path, override_path: Option<&Path>) -> Result<PathBuf> {
    let path = override_path.map_or_else(|| root.join(".comb/service.sock"), Path::to_path_buf);
    let parent = path.parent().ok_or_else(Error::socket_parent_missing)?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let parent = parent.canonicalize()?;
    let name = path
        .file_name()
        .ok_or_else(Error::socket_filename_missing)?;
    let path = parent.join(name);
    // macOS sockaddr_un is the tighter supported platform limit.
    if path.as_os_str().as_encoded_bytes().len() > limits::SOCKET_BYTES {
        return Err(Error::socket_path_too_long());
    }
    Ok(path)
}

/// Server-only admission; clients retain their independent path validation.
pub fn check_socket_output(root: &Path, path: &Path) -> Result<()> {
    let path = socket_path(root, Some(path))?;
    super::socket::directory::check(
        path.parent().ok_or_else(Error::socket_parent_missing)?,
        super::socket::directory::Action::Existing,
    )?;
    Ok(())
}
impl Prepared {
    pub fn bind(self) -> Result<Server> {
        let Self {
            state,
            queries,
            lease,
        } = self;
        state.health.check()?;
        let (listener, socket) = lease.bind()?;
        Ok(Server {
            state,
            queries,
            listener,
            _socket: socket,
        })
    }
}
impl Server {
    /// Called outside Tokio because HTTP engine construction uses blocking I/O.
    pub fn open(root: &Path, options: Options) -> Result<Self> {
        Self::prepare(root, options)?.bind()
    }
    /// Load state and models outside Tokio, before installing CLI signal handlers.
    pub fn prepare(root: &Path, options: Options) -> Result<Prepared> {
        Self::prepare_expected(root, options, None)
    }
    /// Managed startup compares the engine's already-hashed assets to its pinned
    /// identity before activation, including when the derived index was rebuilt.
    pub fn prepare_expected(
        root: &Path,
        options: Options,
        expected: Option<&crate::embedding::Space>,
    ) -> Result<Prepared> {
        Self::validate_options(&options)?;
        options.validate_model(options.embeddings.is_some())?;
        let lease = Self::preflight(root, &options)?;
        let engine = options.embeddings.clone().map(Engine::load).transpose()?;
        Self::prepare_with_engine(root, options, expected, engine, lease, &|_| Ok(()))
    }
    pub(crate) fn prepare_with_engine(
        root: &Path,
        options: Options,
        expected: Option<&crate::embedding::Space>,
        engine: Option<Engine>,
        mut lease: super::socket::Lease,
        check: &impl Fn(&crate::disk::Disk) -> Result<()>,
    ) -> Result<Prepared> {
        options.recheck_model(engine.is_some())?;
        lease.check_path(&socket_path(root, options.socket.as_deref())?)?;
        if expected.is_some() && engine.as_ref().map(Embedder::space) != expected {
            return Err(Error::managed_model_changed());
        }
        let mut comb = Comb::open_checked(root, &|disk| {
            lease.check_disk(disk)?;
            check(disk)
        })?;
        lease.reclaim(&comb.disk)?;
        comb.sync()?;
        Self::from_owned(
            comb,
            options,
            engine.map(|e| Box::new(e) as Box<dyn Embedder>),
            lease,
        )
    }
    /// Observe startup inputs without touching knowledge or removing sockets.
    pub(crate) fn preflight(root: &Path, options: &Options) -> Result<super::socket::Lease> {
        Self::validate_options(options)?;
        super::socket::Lease::acquire(root, socket_path(root, options.socket.as_deref())?)
    }
    pub(crate) fn validate_options(options: &Options) -> Result<()> {
        if options
            .vector_cache_mib
            .is_some_and(|mib| !limits::VECTOR_CACHE_MIB.contains(&mib))
        {
            return Err(Error::invalid_cache_budget());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn from_comb(
        comb: Comb,
        options: Options,
        engine: Option<Box<dyn Embedder>>,
    ) -> Result<Prepared> {
        Self::validate_options(&options)?;
        options.validate_model(engine.is_some())?;
        let mut lease = Self::preflight(&comb.disk.root, &options)?;
        lease.reclaim(&comb.disk)?;
        Self::from_owned(comb, options, engine, lease)
    }
    fn from_owned(
        mut comb: Comb,
        options: Options,
        engine: Option<Box<dyn Embedder>>,
        lease: super::socket::Lease,
    ) -> Result<Prepared> {
        Self::validate_options(&options)?;
        lease.check_disk(&comb.disk)?;
        let health = comb.health.clone();
        let model = model(&mut comb, &options, engine)?;
        let resident = resident_cache(&comb, &options, model.as_ref())?;
        comb.index = comb.index.with_vectors(resident.clone());
        let registry = Registry::open(&comb.disk, health.clone())?;
        let index_path = comb.disk.index_path();
        let publication = comb.publication.clone();
        let (query_queue, queries) = queries::Queue::new(health.clone());
        Ok(Prepared {
            queries,
            lease,
            state: Arc::new(State {
                observations: super::observations::Counters::default(),
                writer: CheckedMutex::new(comb, health.clone()),
                health,
                registry,
                index_path,
                publication,
                resident,
                model,
                reads: Arc::new(Semaphore::new(limits::READ_SLOTS)),
                writes: Arc::new(Semaphore::new(limits::WRITE_SLOTS)),
                queries: query_queue,
                wake: Notify::new(),
            }),
        })
    }
}

fn model(
    comb: &mut Comb,
    options: &Options,
    engine: Option<Box<dyn Embedder>>,
) -> Result<Option<Model>> {
    let Some(mut engine) = engine else {
        return Ok(None);
    };
    comb.embedding_runtime = engine.runtime();
    let lease = comb
        .activate_engine(engine.as_mut(), options.replace_embeddings)
        .map_err(|error| {
            if error.code == crate::ErrorCode::EmbeddingReplaceRequired {
                Error::serve_embedding_replace_required().caused_by(error)
            } else {
                error
            }
        })?;
    if options.retry_failed {
        comb.retry_embedding_failures()?;
    }
    let health = comb.health.clone();
    let query_engine = engine
        .query_engine()
        .map(|engine| CheckedMutex::new(engine, health.clone()));
    Ok(Some(Model {
        runtime: comb.embedding_runtime.clone(),
        lease,
        query_engine,
        engine: CheckedMutex::new(engine, health),
        pending_queries: Arc::new(AtomicUsize::new(0)),
    }))
}

fn resident_cache(
    comb: &Comb,
    options: &Options,
    model: Option<&Model>,
) -> Result<Option<Arc<crate::index::cache::Resident>>> {
    model
        .map(|_| comb.index()?.active_space())
        .transpose()?
        .flatten()
        .map(|(lease, _)| {
            let cache = Arc::new(crate::index::cache::Resident::new(
                lease,
                options
                    .vector_cache_mib
                    .unwrap_or(limits::DEFAULT_VECTOR_CACHE_MIB)
                    * limits::MIB,
                comb.health.clone(),
            )?);
            comb.index()?.warm_vectors(&cache)?;
            Ok(cache)
        })
        .transpose()
}
