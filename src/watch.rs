use std::path::PathBuf;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

pub enum WatchEvent {
    Changed,
}

pub struct FolderWatch {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<WatchEvent>,
    last_change: Mutex<Option<Instant>>,
}

impl FolderWatch {
    pub fn current_folder(path: PathBuf) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.send(WatchEvent::Changed);
                }
            },
            Config::default().with_poll_interval(Duration::from_millis(100)),
        )?;
        watcher.watch(&path, RecursiveMode::NonRecursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
            last_change: Mutex::new(None),
        })
    }

    pub fn take_change(&self) -> bool {
        let mut any = false;
        while self.rx.try_recv().is_ok() {
            any = true;
        }
        any
    }

    /// Drain pending events, then report a change only after `min_interval` of quiet.
    /// Bursts in the 50–100ms range collapse into one `true`.
    pub fn take_change_debounced(&self, min_interval: Duration) -> bool {
        let now = Instant::now();
        let mut last_change = self
            .last_change
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.take_change() {
            *last_change = Some(now);
            return false;
        }
        if let Some(t) = *last_change {
            if now.saturating_duration_since(t) >= min_interval {
                *last_change = None;
                return true;
            }
        }
        false
    }
}
