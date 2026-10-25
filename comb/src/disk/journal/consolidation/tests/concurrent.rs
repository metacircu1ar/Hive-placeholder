use super::*;
use crate::{
    disk::durable::{Durability, System},
    index::Index,
    read::Snapshot,
};
use std::{
    fs::File,
    sync::{Mutex, mpsc},
    time::Duration,
};

struct PausedFinish {
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<bool>>,
}
impl Durability for PausedFinish {
    fn fsync(&self, file: &File) -> io::Result<()> {
        System.fsync(file)
    }
    fn ordering_barrier(&self, file: &File) -> io::Result<()> {
        System.ordering_barrier(file)
    }
    fn full_flush(&self, file: &File) -> io::Result<()> {
        System.full_flush(file)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        System.sync_directory(file)
    }
    fn before_finish(&self) -> Result<()> {
        self.entered.send(()).unwrap();
        if self
            .release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .expect("fixture must release finalization before its deadline")
        {
            return Err(Error::from(io::Error::from_raw_os_error(libc::EIO)).committed());
        }
        Ok(())
    }
}

#[test]
fn concurrent_reader_waits_through_finalization_then_gets_whole_pair_or_failure() {
    for failed in [false, true] {
        let mut fixture = Fixture::new();
        let index = fixture.comb.disk.index_path();
        let gate = fixture.comb.publication.clone();
        let request = fixture.request.clone();
        let (entered, waiting) = mpsc::sync_channel(0);
        let (release, resume) = mpsc::channel();
        fixture.comb.disk.set_durability(Arc::new(PausedFinish {
            entered,
            release: Mutex::new(resume),
        }));
        let writer = std::thread::spawn(move || {
            let result = fixture.apply();
            (fixture, result)
        });
        waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        let (started, starting) = mpsc::sync_channel(0);
        let (done, results) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let index = Index::reader(&index, None).unwrap();
            started.send(()).unwrap();
            let result = Snapshot::pin(index, &gate).and_then(|snapshot| {
                Ok((
                    snapshot.index.document(&request.target)?.hash,
                    snapshot.index.document(&request.note)?.hash,
                    snapshot.index.stats()?.inbox,
                ))
            });
            done.send(result).unwrap();
        });
        starting.recv_timeout(Duration::from_secs(10)).unwrap();
        let early = results.recv_timeout(Duration::from_millis(100));
        release.send(failed).unwrap();
        let (fixture, result) = writer.join().unwrap();
        assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)));
        let observed = results.recv_timeout(Duration::from_secs(10)).unwrap();
        reader.join().unwrap();
        if failed {
            assert!(result.unwrap_err().file_committed);
            assert_eq!(observed.unwrap_err().code, ErrorCode::RecoveryRequired);
        } else {
            let result = result.unwrap();
            assert_eq!(observed.unwrap(), (result.target.hash, result.note.hash, 0));
        }
        drop(fixture.comb);
        assert_eq!(
            Comb::open(fixture.root.path())
                .unwrap()
                .stats()
                .unwrap()
                .inbox,
            0
        );
    }
}
