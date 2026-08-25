use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// A filesystem watcher that emits only real changes after a quiet period.
pub struct PassiveDebouncedWatcher {
    watcher: Option<RecommendedWatcher>,
    worker: Option<JoinHandle<()>>,
}

impl PassiveDebouncedWatcher {
    pub fn new(
        delay: Duration,
        mut callback: impl FnMut(Vec<PathBuf>) + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                return;
            };
            if !matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                return;
            }
            for path in event.paths {
                if sender.send(path).is_err() {
                    break;
                }
            }
        })
        .map_err(io::Error::other)?;
        let worker = thread::Builder::new()
            .name("notist-file-watcher".into())
            .spawn(move || {
                while let Ok(path) = receiver.recv() {
                    let mut paths = BTreeSet::from([path]);
                    loop {
                        match receiver.recv_timeout(delay) {
                            Ok(path) => {
                                paths.insert(path);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                callback(paths.into_iter().collect());
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                callback(paths.into_iter().collect());
                                return;
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            watcher: Some(watcher),
            worker: Some(worker),
        })
    }

    pub fn watch_recursive(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.watcher
            .as_mut()
            .expect("watcher is unavailable only while dropping")
            .watch(path.as_ref(), RecursiveMode::Recursive)
            .map_err(io::Error::other)
    }
}

impl Drop for PassiveDebouncedWatcher {
    fn drop(&mut self) {
        drop(self.watcher.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::PassiveDebouncedWatcher;

    #[test]
    fn reads_do_not_trigger_changes_but_writes_do() {
        let directory = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let path = directory.path().join("watched.not");
        fs::write(&path, "before").unwrap();

        let (sender, receiver) = mpsc::channel();
        let mut watcher = PassiveDebouncedWatcher::new(Duration::from_millis(250), move |paths| {
            sender.send(paths).unwrap();
        })
        .unwrap();
        watcher.watch_recursive(directory.path()).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "before");
        assert!(
            receiver.recv_timeout(Duration::from_millis(750)).is_err(),
            "reading an existing file must not produce a change callback"
        );

        fs::write(&path, "after").unwrap();
        let paths = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("writing a watched file should produce a change callback");
        assert!(paths.iter().any(|changed| changed == &path), "{paths:?}");
    }
}
