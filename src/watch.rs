use std::path::PathBuf;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::event::{EventKind, MetadataKind, ModifyKind};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

pub enum WatchEvent {
    Changed,
}

pub struct FolderWatch {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<WatchEvent>,
    last_change: Mutex<Option<Instant>>,
}

/// True for events that mean directory contents (or meaningful metadata) changed.
/// Access / atime-only events are ignored — our own `list_dir` readdir would otherwise
/// feed an infinite reload loop through the watch poll.
fn is_substantive_change(kind: EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Name(_))
        | EventKind::Modify(ModifyKind::Any)
        | EventKind::Modify(ModifyKind::Other) => true,
        EventKind::Modify(ModifyKind::Metadata(meta)) => {
            !matches!(meta, MetadataKind::AccessTime | MetadataKind::Extended)
        }
        EventKind::Access(_) | EventKind::Other => false,
    }
}

impl FolderWatch {
    pub fn current_folder(path: PathBuf) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else {
                    return;
                };
                if is_substantive_change(event.kind) {
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

    /// Drop pending events and debounce state. Call after our own listing so any
    /// residual notify noise from the scan cannot schedule another reload.
    pub fn acknowledge(&self) {
        let _ = self.take_change();
        let mut last_change = self
            .last_change
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *last_change = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, RemoveKind, RenameMode};

    #[test]
    fn access_events_are_not_substantive() {
        assert!(!is_substantive_change(EventKind::Access(AccessKind::Any)));
        assert!(!is_substantive_change(EventKind::Access(AccessKind::Read)));
        assert!(!is_substantive_change(EventKind::Access(AccessKind::Open(
            notify::event::AccessMode::Any
        ))));
        assert!(!is_substantive_change(EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::AccessTime)
        )));
        assert!(!is_substantive_change(EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Extended)
        )));
        assert!(!is_substantive_change(EventKind::Other));
    }

    #[test]
    fn content_events_are_substantive() {
        assert!(is_substantive_change(EventKind::Create(CreateKind::Any)));
        assert!(is_substantive_change(EventKind::Remove(RemoveKind::Any)));
        assert!(is_substantive_change(EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(is_substantive_change(EventKind::Modify(ModifyKind::Name(
            RenameMode::Any
        ))));
        assert!(is_substantive_change(EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::WriteTime)
        )));
        assert!(is_substantive_change(EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Permissions)
        )));
        assert!(is_substantive_change(EventKind::Any));
    }
}
