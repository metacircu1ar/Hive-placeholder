//! The atomic aside rename carries its original directory and parent identities.
use super::{Error, Path, PathBuf, Result, exists, fs, invalid, state};
use std::os::unix::fs::MetadataExt;
const PREFIX: &str = ".hive-retiring-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::launch::image) struct Identity {
    pub device: u64,
    pub inode: u64,
}
impl Identity {
    pub(in crate::launch::image) fn at(path: &Path) -> Result<Self> {
        state::check_path(path, true)?;
        let metadata = fs::symlink_metadata(path).map_err(Error::state_io_from)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    pub(in crate::launch::image) fn directory(self) -> atomic_fs::DirectoryIdentity {
        atomic_fs::DirectoryIdentity::new(self.device, self.inode)
    }
}
/// Where a retirement's device numbers come from. Inodes identify its
/// directories either way; a remount can renumber a device without changing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Devices {
    /// Read from the directories by this process: compared exactly.
    Live,
    /// Read back from an aside's name, possibly written before a remount.
    Named,
}
#[derive(Clone)]
pub(in crate::launch::image) struct Retirement {
    pub original: PathBuf,
    pub aside: PathBuf,
    pub parent: Identity,
    pub tree: Identity,
    devices: Devices,
}
impl Retirement {
    pub(in crate::launch::image) fn capture(original: &Path) -> Result<Self> {
        let name = original
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| image_name(n))
            .ok_or_else(invalid)?;
        let root = original.parent().ok_or_else(invalid)?;
        let parent = Identity::at(root)?;
        let tree = Identity::at(original)?;
        // A directory on another filesystem is a mount point, never an image.
        if tree.device != parent.device {
            return Err(invalid());
        }
        Ok(Self {
            original: original.into(),
            aside: root.join(format!(
                "{PREFIX}{name}.{:016x}{:016x}{:016x}{:016x}",
                parent.device, parent.inode, tree.device, tree.inode
            )),
            parent,
            tree,
            devices: Devices::Live,
        })
    }
    pub(in crate::launch::image) fn parse(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(invalid)?;
        let (original, hex) = name
            .strip_prefix(PREFIX)
            .and_then(|n| n.rsplit_once('.'))
            .ok_or_else(invalid)?;
        if !image_name(original)
            || hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid());
        }
        let number = |start| {
            u64::from_str_radix(&hex[start..start + 16], 16)
                .map_err(|cause| invalid().with_source(cause))
        };
        let root = path.parent().ok_or_else(invalid)?;
        let parent = Identity {
            device: number(0)?,
            inode: number(16)?,
        };
        let tree = Identity {
            device: number(32)?,
            inode: number(48)?,
        };
        // Captured together, from one filesystem.
        if parent.device != tree.device {
            return Err(invalid());
        }
        Ok(Self {
            original: root.join(original),
            aside: path.into(),
            parent,
            tree,
            devices: Devices::Named,
        })
    }
    pub(in crate::launch::image) fn root(&self) -> Result<&Path> {
        self.original.parent().ok_or_else(invalid)
    }
    /// Check the parent directory; returns its live identity.
    pub(in crate::launch::image) fn check_parent(&self) -> Result<Identity> {
        let live = Identity::at(self.root()?)?;
        match self.devices {
            Devices::Live if live == self.parent => Ok(live),
            Devices::Named if live.inode == self.parent.inode => {
                if live.device != self.parent.device {
                    tracing::warn!(name: "launch.retirement_device_changed",
                         decision = "match_inodes");
                }
                Ok(live)
            }
            _ => Err(invalid()),
        }
    }
    /// Check the parent and the retired directory at `tree`; returns the
    /// directory's live identity. A named retirement's directory must still
    /// share its parent's filesystem.
    pub(in crate::launch::image) fn check(&self, tree: &Path) -> Result<Identity> {
        let parent = self.check_parent()?;
        let live = Identity::at(tree)?;
        let same = match self.devices {
            Devices::Live => live == self.tree,
            Devices::Named => live.inode == self.tree.inode && live.device == parent.device,
        };
        if !same {
            return Err(invalid());
        }
        Ok(live)
    }
    pub(in crate::launch::image) fn collisions(&self) -> Result<()> {
        let matching = matching_asides(&self.original)?;
        if matching.len() > 1
            || matching.iter().any(|path| *path != self.aside)
            || exists(&self.original)? && exists(&self.aside)?
        {
            return Err(invalid());
        }
        Ok(())
    }
}
pub(in crate::launch::image) fn image_name(name: &str) -> bool {
    name.strip_prefix("hive-launch-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 64
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}
pub(in crate::launch::image) fn is_aside(name: &str) -> bool {
    name.starts_with(PREFIX)
}
pub(in crate::launch::image) fn matching_asides(original: &Path) -> Result<Vec<PathBuf>> {
    let name = original
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| image_name(n))
        .ok_or_else(invalid)?;
    let prefix = format!("{PREFIX}{name}.");
    let mut matching = Vec::new();
    for entry in
        fs::read_dir(original.parent().ok_or_else(invalid)?).map_err(Error::state_io_from)?
    {
        let entry = entry.map_err(Error::state_io_from)?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(&prefix))
        {
            matching.push(entry.path());
        }
    }
    Ok(matching)
}
