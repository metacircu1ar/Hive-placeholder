use std::{
    ffi::OsStr,
    fs,
    io::{self, Read},
    ops::{Deref, DerefMut},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    thread::{self, JoinHandle},
};

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"HIVE_") {
            command.env_remove(name);
        }
    }
    command
}

pub struct ChildGuard(Option<Child>);
impl ChildGuard {
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }
    pub fn wait_with_stdout(mut self) -> io::Result<Output> {
        let child = self.0.as_mut().unwrap();
        assert!(
            child.stderr.is_none(),
            "stdout capture requires inherited stderr"
        );
        drop(child.stdin.take());
        let mut stdout = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            pipe.read_to_end(&mut stdout)?;
        }
        let status = child.wait()?;
        Ok(Output {
            status,
            stdout,
            stderr: Vec::new(),
        })
    }
}
impl Deref for ChildGuard {
    type Target = Child;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}
impl DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct PermissionsGuard {
    path: PathBuf,
    previous: fs::Permissions,
}
impl PermissionsGuard {
    pub fn set(path: impl AsRef<Path>, mode: u32) -> Self {
        let path = path.as_ref().to_owned();
        let previous = fs::metadata(&path).unwrap().permissions();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        Self { path, previous }
    }
}
impl Drop for PermissionsGuard {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, self.previous.clone());
    }
}

pub struct ReleaseFile(PathBuf);
impl ReleaseFile {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self(path.as_ref().to_owned())
    }
    pub fn release(&self) {
        fs::write(&self.0, "").unwrap();
    }
}
impl Drop for ReleaseFile {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, "");
    }
}

pub struct ThreadGuard<T> {
    thread: Option<JoinHandle<T>>,
    release: Option<Box<dyn FnOnce()>>,
}
impl<T> ThreadGuard<T> {
    pub fn new(thread: JoinHandle<T>, release: impl FnOnce() + 'static) -> Self {
        Self {
            thread: Some(thread),
            release: Some(Box::new(release)),
        }
    }
    pub fn join(mut self) -> thread::Result<T> {
        self.release.take();
        self.thread.take().unwrap().join()
    }
}
impl<T> Drop for ThreadGuard<T> {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
