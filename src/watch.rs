//! Filesystem watching for the Autoreload feature.

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<()>,
    path: PathBuf,
}

impl FileWatcher {
    /// Watches the containing directory rather than the file itself: many
    /// tools save by writing a temporary file and renaming over the original,
    /// which drops a watch registered on the inode.
    pub fn new(path: &Path) -> Option<Self> {
        let dir = path.parent()?.to_path_buf();
        let target = path.to_path_buf();
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if event.paths.contains(&target) {
                    let _ = tx.send(());
                }
            }
        })
        .ok()?;
        watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
        Some(Self {
            _watcher: watcher,
            rx,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True if the watched file changed since the last call.
    pub fn take_change(&self) -> bool {
        let mut changed = false;
        while let Ok(()) = self.rx.try_recv() {
            changed = true;
        }
        changed
    }
}
