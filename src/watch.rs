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

    #[test]
    fn substantive_kind_truth_table() {
        use notify::event::{AccessMode, RemoveKind, RenameMode};
        let cases = [
            (EventKind::Any, true),
            (EventKind::Create(CreateKind::Folder), true),
            (EventKind::Remove(RemoveKind::File), true),
            (EventKind::Modify(ModifyKind::Data(DataChange::Size)), true),
            (EventKind::Modify(ModifyKind::Name(RenameMode::Any)), true),
            (
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
                true,
            ),
            (
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
                true,
            ),
            (
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)),
                false,
            ),
            (
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Extended)),
                false,
            ),
            (EventKind::Access(AccessKind::Read), false),
            (EventKind::Access(AccessKind::Open(AccessMode::Any)), false),
            (EventKind::Access(AccessKind::Close(AccessMode::Any)), false),
            (EventKind::Other, false),
        ];
        for (kind, want) in cases {
            assert_eq!(is_substantive(kind), want, "kind {kind:?}");
        }
    }

    fn unique_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ply-watch-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn debounce_without_any_change_is_false() {
        let dir = unique_dir("quiet");
        let watch = FolderWatch::current_folder(dir.clone()).expect("watch must start");
        assert!(!watch.take_change_debounced(Duration::from_secs(10)));
        assert!(!watch.take_change());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debounce_collapses_a_change_then_fires_once_quiet() {
        let dir = unique_dir("burst");
        let watch = FolderWatch::current_folder(dir.clone()).expect("watch must start");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();

        // With a zero quiet window: the first call swallows the pending event
        // (returns false), and a later call with no new events reports true.
        let mut fired = false;
        for _ in 0..600 {
            if watch.take_change_debounced(Duration::ZERO) {
                fired = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(fired, "the debounce must report the quiet change once");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
