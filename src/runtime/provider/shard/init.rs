//! Shared shard and driver construction with explicit channel ownership.
use super::{Cmd, Error, Occupancy, Pool, Result, Rpc, RuntimeLease, Shard, ShardLoop, Started};
use crate::runtime::auth_fallback::signals::CodexLimits;
use std::{
    collections::{HashMap, HashSet},
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize},
    },
    time::Instant,
};
use tokio::sync::{Notify, OnceCell, mpsc};

pub(super) struct ShardIdentity<'a> {
    pub id: String,
    pub auth: &'a str,
    pub process_options: &'a str,
    pub key: Option<&'a crate::state::KeyAuthority>,
    pub backend: &'a super::super::Backend,
    pub compatibility: Option<&'a crate::provider_support::Evidence>,
    pub lease: &'a Arc<RuntimeLease>,
}
pub(super) struct ShardInputs {
    pub normal: mpsc::Sender<Cmd>,
    pub priority: mpsc::Sender<Cmd>,
    pub essential: mpsc::Sender<Cmd>,
    pub wake: UnixStream,
}
impl Shard {
    pub(super) fn new(
        identity: ShardIdentity<'_>,
        inputs: ShardInputs,
        starting: bool,
        last_used: impl FnOnce() -> u64,
    ) -> Self {
        // Read the pool clock at its initialization boundary, after creation time.
        Self {
            id: identity.id,
            auth: identity.auth.into(),
            process_options: identity.process_options.into(),
            key: identity.key.cloned(),
            backend: identity.backend.clone(),
            compatibility: identity.compatibility.cloned(),
            created: Instant::now(),
            occupancy: Mutex::new(Occupancy::default()),
            empty_attach_check: AtomicBool::new(false),
            abandoned_attach: AtomicBool::new(false),
            ready_since: OnceLock::new(),
            lease: identity.lease.clone(),
            starting: AtomicBool::new(starting),
            steering_unsupported: AtomicBool::new(false),
            last_used: AtomicU64::new(last_used()),
            normal: inputs.normal,
            priority: inputs.priority,
            essential: inputs.essential,
            wake: inputs.wake,
            draining: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            final_events: AtomicBool::new(false),
            capacity_released: AtomicBool::new(false),
            process: Mutex::new(None),
            stop: AtomicBool::new(false),
            catalog: OnceCell::new(),
            binary_identity: OnceLock::new(),
            report: Mutex::new(None),
            done: Notify::new(),
        }
    }
}

pub(crate) struct ShardChannels {
    pub wake: UnixStream,
    pub normal: mpsc::Receiver<Cmd>,
    pub priority: mpsc::Receiver<Cmd>,
    pub essential: mpsc::Receiver<Cmd>,
}
pub(super) fn channels(capacities: [usize; 3]) -> Result<(ShardInputs, ShardChannels)> {
    let [normal_capacity, priority_capacity, essential_capacity] = capacities;
    let (normal, nrx) = mpsc::channel(normal_capacity);
    let (priority, prx) = mpsc::channel(priority_capacity);
    let (essential, erx) = mpsc::channel(essential_capacity);
    let (writer, reader) = {
        let _fork = crate::os::fork_guard().map_err(Error::state_io_from)?;
        UnixStream::pair().map_err(Error::state_io_from)?
    };
    writer.set_nonblocking(true).map_err(Error::state_io_from)?;
    reader.set_nonblocking(true).map_err(Error::state_io_from)?;
    Ok((
        ShardInputs {
            normal,
            priority,
            essential,
            wake: writer,
        },
        ShardChannels {
            normal: nrx,
            priority: prx,
            essential: erx,
            wake: reader,
        },
    ))
}
impl<'a> ShardLoop<'a> {
    pub(super) fn new(
        pool: &'a Pool,
        shard: &'a Arc<Shard>,
        rpc: &'a mut Rpc,
        started: &'a Started,
        inputs: &'a mut ShardChannels,
    ) -> Self {
        Self {
            pool,
            shard,
            rpc,
            started,
            wake: &mut inputs.wake,
            normal: &mut inputs.normal,
            priority: &mut inputs.priority,
            essential: &mut inputs.essential,
            limits: CodexLimits::default(),
            notices: HashSet::new(),
            routes: HashMap::new(),
            threads: HashMap::new(),
            pending: HashMap::new(),
            bytes: Arc::new(AtomicUsize::new(0)),
            release_replies: Vec::new(),
            interrupted: HashSet::new(),
            sink_threads: HashSet::new(),
            closing: None,
        }
    }
}
