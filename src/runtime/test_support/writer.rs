//! A blocking writer hook whose release signal is also sent during unwinding.
pub(crate) struct WriterGate(Option<std::sync::mpsc::Sender<()>>);
impl WriterGate {
    pub fn new() -> (Self, std::sync::mpsc::Receiver<()>) {
        let (release, released) = std::sync::mpsc::channel();
        (Self(Some(release)), released)
    }
    pub fn release(mut self) {
        self.0.take().unwrap().send(()).unwrap();
    }
    fn send(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}
impl Drop for WriterGate {
    fn drop(&mut self) {
        self.send();
    }
}
