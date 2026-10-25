//! Spawn provider transports while retaining launch and descriptor ownership.
use super::{
    AsFd, AsRawFd, BorrowedFd, Command, Error, HashMap, Outbox, Pipes, Result, Rpc, Stdio, os,
};
use crate::launch::{self, LauncherCache};
use crate::runtime::provider::{Backend, claude::Codec};
use std::os::fd::RawFd;

// The launcher descriptor map uses a negative slot for an absent inherited descriptor.
const ABSENT_INHERITED_DESCRIPTOR: RawFd = -1;

impl Rpc {
    // Own the descriptor list until spawn completes; its callback only borrows live owners.
    #[cfg(test)]
    pub(crate) fn start(
        launcher: &LauncherCache,
        cmd: Command,
        inherited: Vec<Option<BorrowedFd<'_>>>,
    ) -> Result<Self> {
        Self::start_with(launcher, cmd, Backend::Codex, inherited)
    }
    #[cfg(test)]
    pub(crate) fn start_with(
        launcher: &LauncherCache,
        cmd: Command,
        backend: Backend,
        inherited: Vec<Option<BorrowedFd<'_>>>,
    ) -> Result<Self> {
        Self::start_authorized(launcher, cmd, backend, inherited, |_| Ok(()))
    }
    // Keep the guard, inherited descriptor borrows and child assembly in one scope
    // until pipe setup succeeds; failure drops the fully owned Rpc here.
    #[expect(
        clippy::expect_used,
        reason = "The successful launcher preserves all three Stdio::piped descriptors \
            configured here."
    )]
    pub(crate) fn start_authorized(
        launcher: &LauncherCache,
        mut cmd: Command,
        backend: Backend,
        inherited: Vec<Option<BorrowedFd<'_>>>,
        authorize: impl FnOnce(u32) -> Result<()>,
    ) -> Result<Self> {
        let capability = match &backend {
            Backend::Claude(config) => config.capability.clone(),
            Backend::Codex => None,
        };
        let mut claude = configure_backend(launcher, &mut cmd, backend)?;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let raw_descriptors: Vec<_> = inherited
            .iter()
            .map(|fd| fd.map(|fd| fd.as_raw_fd()))
            .collect();
        let descriptors: Vec<_> = raw_descriptors
            .into_iter()
            .map(|fd| fd.unwrap_or(ABSENT_INHERITED_DESCRIPTOR))
            .collect();
        let child = launch::spawn(launcher, &cmd, true, &descriptors, |pid| {
            authorize(pid)?;
            if let Some(capability) = &capability {
                capability.check_launch()?;
            }
            Ok(())
        });
        let mut child = child?;
        if let Some(codec) = &mut claude {
            codec.guard.bind(child.id());
        }
        let stdout = child.stdout.take().expect("provider stdout is piped");
        let stderr = child.stderr.take().expect("provider stderr is piped");
        let rpc = Self {
            initialization: None,
            claude,
            outbox: Outbox::default(),
            pipes: Pipes {
                child,
                stdout,
                stderr,
                stdout_eof: false,
                stderr_eof: false,
                stderr_bytes: 0,
                buffer: Vec::new(),
            },
            pending: super::ByteQueue::default(),
            next_id: 0,
            closed: None,
            threads: HashMap::new(),
            requests: HashMap::new(),
            opening: 0,
            account_identity: None,
            protocol_uncertain: false,
            has_quarantined: false,
            expected_method: crate::config::Method::Subscription,
            private_bootstrap: false,
            live: None,
            deltas: super::super::live_tap::Deltas::default(),
            read_seq: 0,
        };
        rpc.configure_pipes()?;
        Ok(rpc)
    }
    #[expect(
        clippy::expect_used,
        reason = "Only start_authorized calls this before the piped stdin can be taken for \
            shutdown."
    )]
    fn configure_pipes(&self) -> Result<()> {
        for fd in [
            self.pipes.stdout.as_fd(),
            self.pipes.stderr.as_fd(),
            self.pipes
                .child
                .stdin
                .as_ref()
                .expect("provider stdin is piped")
                .as_fd(),
        ] {
            if let Err(cause) = os::nonblocking(fd) {
                return Err(
                    Error::pipe_setup_failed("Cannot configure provider pipes.").with_source(cause)
                );
            }
        }
        Ok(())
    }
}

fn configure_backend(
    launcher: &LauncherCache,
    cmd: &mut Command,
    backend: Backend,
) -> Result<Option<Codec>> {
    let claude = match backend {
        Backend::Codex => {
            cmd.arg("app-server");
            None
        }
        Backend::Claude(config) => {
            let guard = config.configure(cmd, launcher).map_err(|cause| {
                Error::spawn_failed("Cannot prepare Claude Code launch.").with_source(cause)
            })?;
            let fork = config.history.fork_source().is_some();
            let mut codec = Codec::new(config, guard);
            if fork {
                let home = cmd
                    .get_envs()
                    .find(|(key, _)| *key == "CLAUDE_CONFIG_DIR")
                    .and_then(|(_, value)| value)
                    .ok_or_else(|| Error::state_inconsistent("Missing Claude credential home."))?;
                codec.prepare_branch(std::path::Path::new(home))?;
            }
            Some(codec)
        }
    };
    Ok(claude)
}
