use std::path::PathBuf;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::event::{EventKind, MetadataKind, ModifyKind};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

pub enum WatchEvent {
    Changed,
}

/// Whether an event reflects a real change to the folder's contents.
///
/// Listing a folder updates access times, which the watcher reports back as a
/// change; treating those as substantive makes reload retrigger itself forever.
fn is_substantive(kind: EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        EventKind::Modify(ModifyKind::Metadata(meta)) => {
            !matches!(meta, MetadataKind::AccessTime | MetadataKind::Extended)
        }
        EventKind::Modify(_) => true,
        EventKind::Access(_) | EventKind::Other => false,
    }
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
                if let Ok(event) = res
                    && is_substantive(event.kind)
                {
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
        if let Some(t) = *last_change
            && now.saturating_duration_since(t) >= min_interval
        {
            *last_change = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange};

    #[test]
    fn access_events_are_ignored() {
        assert!(!is_substantive(EventKind::Access(AccessKind::Read)));
        assert!(!is_substantive(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::AccessTime
        ))));
    }

    #[test]
    fn content_events_are_substantive() {
        assert!(is_substantive(EventKind::Create(CreateKind::File)));
        assert!(is_substantive(EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
    }
}
