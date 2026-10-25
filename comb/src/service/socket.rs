//! A socket lease spans startup, serving and exact-entry cleanup.
pub(crate) mod directory;
mod files;
mod listener;
mod record;
use super::auth::with_cleanup;
use crate::{Error, Result, disk::Disk};
use files::{Lock, Parent};
use record::{Entry, Node, Receipt, Root};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) struct Lease {
    pub(super) path: PathBuf,
    parent: Parent,
    lock: Lock,
    lock_name: String,
    receipt_name: String,
    root: Root,
    prior: Option<Receipt>,
}
impl Lease {
    pub fn acquire(root: &Path, path: PathBuf) -> Result<Self> {
        hive_setup_support::names::check_output_name(&path)?;
        let root = Root::read(root)?;
        let parent = Parent::open(path.parent().ok_or_else(Error::socket_parent_missing)?)?;
        let hash = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
        let lock_name = format!(".comb-socket-{hash}.lock");
        let receipt_name = format!(".comb-socket-{hash}.json");
        let lock = Lock::acquire(&parent, &lock_name)?;
        let prior = record::read(&parent, &receipt_name)?;
        let lease = Self {
            path,
            parent,
            lock,
            lock_name,
            receipt_name,
            root,
            prior,
        };
        lease.check()?;
        Ok(lease)
    }
    fn check(&self) -> Result<()> {
        self.parent.check()?;
        self.lock.check(&self.parent, &self.lock_name)?;
        self.root.check()?;
        if record::read(&self.parent, &self.receipt_name)? != self.prior {
            return Err(Error::socket_in_use());
        }
        if let Some(prior) = &self.prior
            && (prior.path != self.path
                || prior.parent != self.parent.node
                || prior.root != self.root)
        {
            return Err(Error::socket_in_use());
        }
        if let Some(entry) = self.entry()?
            && self.prior.as_ref().map(|p| &p.entry) != Some(&entry)
        {
            return Err(Error::socket_in_use());
        }
        Ok(())
    }
    pub fn check_disk(&self, disk: &Disk) -> Result<()> {
        self.root.check_disk(disk)?;
        self.check()
    }
    pub fn check_path(&self, path: &Path) -> Result<()> {
        if path != self.path {
            return Err(Error::socket_in_use());
        }
        self.check()
    }
    pub fn reclaim(&mut self, disk: &Disk) -> Result<()> {
        self.check_disk(disk)?;
        if self.entry()?.is_some() {
            self.parent.remove(self.name()?)?;
        }
        if self.prior.is_some() {
            self.parent.remove(self.receipt_name.as_ref())?;
        }
        self.prior = None;
        Ok(())
    }
    fn name(&self) -> Result<&std::ffi::OsStr> {
        self.path
            .file_name()
            .ok_or_else(Error::socket_filename_missing)
    }
    fn entry(&self) -> Result<Option<Entry>> {
        self.parent
            .stat(self.name()?)?
            .as_ref()
            .map(Entry::socket)
            .transpose()
    }
    pub(super) fn bind(self) -> Result<(std::os::unix::net::UnixListener, Guard)> {
        self.bind_with(&atomic_fs::DefaultBarriers)
    }
    fn bind_with(
        self,
        barriers: &impl atomic_fs::Barriers,
    ) -> Result<(std::os::unix::net::UnixListener, Guard)> {
        self.check()?;
        let output = atomic_fs::PreparedCreate::new(
            self.path.with_file_name(&self.receipt_name),
            (hive_setup_support::names::PUBLICATION_OPTIONS).with_prefix(".comb-tmp-"),
        )
        .map_err(publication)?;
        let prepared = output
            .parent_metadata()
            .map_err(Error::from)
            .and_then(|meta| {
                if !self.parent.node.matches_metadata(&meta) {
                    return Err(Error::unsafe_directory());
                }
                self.open_listener()
            });
        let (listener, entry) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Err(with_cleanup(error, output.cancel().map_err(publication))),
        };
        let receipt = Receipt {
            path: self.path.clone(),
            parent: self.parent.node.clone(),
            root: self.root.clone(),
            entry,
        };
        let mut guard = Guard {
            lease: self,
            receipt,
            pending: true,
        };
        let result = match crate::error::json_bytes(&guard.receipt) {
            Ok(bytes) => output
                .publish_with(
                    &bytes,
                    &Publication {
                        guard: &guard,
                        inner: barriers,
                    },
                )
                .map_err(publication),
            Err(error) => Err(with_cleanup(error, output.cancel().map_err(publication))),
        };
        if let Err(error) = result.and_then(|()| guard.check_entry()) {
            drop(listener);
            return Err(with_cleanup(error, guard.cleanup()));
        }
        Ok((listener, guard))
    }
    fn open_listener(&self) -> Result<(std::os::unix::net::UnixListener, Entry)> {
        self.parent.check()?;
        let listener = listener::bound(&self.path)?;
        let observed = self
            .parent
            .stat(self.name()?)
            .and_then(|stat| stat.ok_or_else(Error::socket_in_use));
        let bound = match observed {
            Ok(stat) => Node::of(&stat),
            Err(error) => {
                drop(listener);
                return Err(with_cleanup(error, Err(Error::socket_in_use())));
            }
        };
        let configured = (|| {
            #[cfg(test)]
            crate::disk::publication_hooks::step(&self.path, atomic_fs::Step::Written)?;
            self.parent.check()?;
            self.parent.private_socket(self.name()?, &bound)?;
            let stat = self
                .parent
                .stat(self.name()?)?
                .ok_or_else(Error::socket_in_use)?;
            if Node::of(&stat) != bound {
                return Err(Error::socket_in_use());
            }
            let entry = Entry::socket(&stat)?;
            listener.set_nonblocking(true)?;
            #[cfg(test)]
            crate::disk::publication_hooks::step(&self.path, atomic_fs::Step::BeforePublish)?;
            listener::listen(&listener)?;
            Ok(entry)
        })();
        match configured {
            Ok(entry) => Ok((listener, entry)),
            Err(error) => {
                drop(listener);
                Err(with_cleanup(error, self.cleanup_unpublished(&bound)))
            }
        }
    }
    fn cleanup_unpublished(&self, expected: &Node) -> Result<()> {
        self.parent.check()?;
        self.lock.check(&self.parent, &self.lock_name)?;
        if let Some(stat) = self.parent.stat(self.name()?)? {
            if &Node::of(&stat) != expected {
                return Err(Error::socket_in_use());
            }
            self.parent.remove(self.name()?)?;
        }
        Ok(())
    }
}
pub(super) struct Guard {
    lease: Lease,
    receipt: Receipt,
    pending: bool,
}
impl Guard {
    fn check_entry(&self) -> Result<()> {
        self.lease.parent.check()?;
        self.lease
            .lock
            .check(&self.lease.parent, &self.lease.lock_name)?;
        self.lease.root.check()?;
        if self.lease.entry()?.as_ref() != Some(&self.receipt.entry) {
            return Err(Error::socket_in_use());
        }
        Ok(())
    }
    pub fn cleanup(&mut self) -> Result<()> {
        self.pending = false;
        self.lease.parent.check()?;
        self.lease
            .lock
            .check(&self.lease.parent, &self.lease.lock_name)?;
        let stored = record::read(&self.lease.parent, &self.lease.receipt_name)?;
        if stored.as_ref().is_some_and(|r| r != &self.receipt) {
            return Err(Error::socket_in_use());
        }
        if let Some(entry) = self.lease.entry()? {
            if entry != self.receipt.entry {
                return Err(Error::socket_in_use());
            }
            self.lease.parent.remove(self.lease.name()?)?;
        }
        if stored.is_some() {
            self.lease.parent.remove(self.lease.receipt_name.as_ref())?;
        }
        Ok(())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.pending
            && let Err(error) = self.cleanup()
        {
            tracing::error!(name: "comb.socket_cleanup_failed",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_ownership_evidence");
        }
    }
}
fn publication(error: atomic_fs::Error) -> Error {
    let mut native = Error::from(std::io::Error::from(error.io_error().kind()));
    native.file_committed = error.published();
    native.caused_by(error)
}

#[cfg(test)]
mod tests;

struct Publication<'a, B> {
    guard: &'a Guard,
    inner: &'a B,
}
impl<B: atomic_fs::Barriers> atomic_fs::Barriers for Publication<'_, B> {
    fn sync_file(&self, file: &std::fs::File) -> std::io::Result<()> {
        self.inner.sync_file(file)
    }
    fn sync_directory(&self, file: &std::fs::File) -> std::io::Result<()> {
        self.inner.sync_directory(file)
    }
    fn commit(&self, file: &std::fs::File) -> std::io::Result<()> {
        self.inner.commit(file)
    }
    fn observe(&self, step: atomic_fs::Step) -> std::io::Result<()> {
        self.inner.observe(step)?;
        if step == atomic_fs::Step::BeforePublish {
            self.guard.check_entry().map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}
