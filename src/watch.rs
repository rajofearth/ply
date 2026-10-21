use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, mpsc};
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

/// Where the persistent watcher's events currently belong: the folder it was
/// pointed at most recently and the channel of the [`FolderWatch`] that owns
/// it. The watcher callback reads this to attribute each delivery, and a
/// folder switch writes it before re-pointing the OS watch, so anything still
/// in flight from the previous folder is unmatched and never fires a reload
/// into the new one. `generation` is bumped on every construction, so the
/// outgoing instance's `Drop` can tell replacement (do nothing) from being
/// the last owner (release the OS watch, e.g. navigating to Home).
struct WatchTarget {
    generation: u64,
    path: Option<PathBuf>,
    tx: Option<mpsc::Sender<WatchEvent>>,
}

/// The watcher itself, created once for the whole session and kept across
/// navigations: a folder switch re-points it instead of paying a fresh
/// watcher's half-second poll-cycle warmup (and its per-folder window of
/// unheard events) on every navigation. Kept separate from [`TARGET`] so the
/// event callback always takes a lock no folder switch holds across
/// `watch`/`unwatch`.
static WATCHER: OnceLock<Mutex<Option<RecommendedWatcher>>> = OnceLock::new();
static TARGET: OnceLock<Mutex<WatchTarget>> = OnceLock::new();

fn new_target() -> WatchTarget {
    WatchTarget {
        generation: 0,
        path: None,
        tx: None,
    }
}

pub struct FolderWatch {
    path: PathBuf,
    generation: u64,
    rx: mpsc::Receiver<WatchEvent>,
    last_change: Mutex<Option<Instant>>,
}

impl FolderWatch {
    pub fn current_folder(path: PathBuf) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let slot = WATCHER.get_or_init(|| Mutex::new(None));
        let mut watch = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if watch.is_none() {
            *watch = Some(RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| {
                    if let Ok(event) = res
                        && is_substantive(event.kind)
                        && let Some(target) = TARGET.get()
                    {
                        let target = target.lock().unwrap_or_else(|p| p.into_inner());
                        if let (Some(current), Some(tx)) = (&target.path, &target.tx)
                            && (event.paths.is_empty()
                                || event.paths.iter().any(|p| p.starts_with(current)))
                        {
                            let _ = tx.send(WatchEvent::Changed);
                        }
                    }
                },
                Config::default().with_poll_interval(Duration::from_millis(100)),
            )?);
        }
        let watch = watch.as_mut().expect("watcher was just ensured");
        let target_cell = TARGET.get_or_init(|| Mutex::new(new_target()));
        let (generation, old_path, old_tx) = {
            let mut target = target_cell.lock().unwrap_or_else(|p| p.into_inner());
            target.generation += 1;
            (
                target.generation,
                target.path.replace(path.clone()),
                target.tx.replace(tx),
            )
        };
        let same = old_path
            .as_ref()
            .is_some_and(|p| p.as_path() == path.as_path());
        if !same {
            // Start watching the new folder before dropping the old watch, so
            // a change arriving between the two is never lost.
            let started = watch.watch(&path, RecursiveMode::NonRecursive);
            if let Err(err) = started {
                let mut target = target_cell.lock().unwrap_or_else(|p| p.into_inner());
                target.generation = generation - 1;
                target.path = old_path;
                target.tx = old_tx;
                return Err(err.into());
            }
            if let Some(old_path) = &old_path {
                let _ = watch.unwatch(old_path);
            }
        }
        Ok(Self {
            path,
            generation,
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

impl Drop for FolderWatch {
    fn drop(&mut self) {
        let Some(target_cell) = TARGET.get() else {
            return;
        };
        let mut target = target_cell.lock().unwrap_or_else(|p| p.into_inner());
        // A replaced instance holds a stale identity: its folder was already
        // handed to the next one. Only the still-current owner releases.
        if target.generation != self.generation {
            return;
        }
        target.path = None;
        drop(target);
        if let Some(slot) = WATCHER.get() {
            let mut watch = slot.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(watch) = watch.as_mut() {
                let _ = watch.unwatch(&self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange};

    /// The two live-folder tests ride the same session-global watcher, and
    /// each one re-points it, so they must not overlap when the harness runs
    /// test threads in parallel.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

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
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = unique_dir("quiet");
        let watch = FolderWatch::current_folder(dir.clone()).expect("watch must start");
        assert!(!watch.take_change_debounced(Duration::from_secs(10)));
        assert!(!watch.take_change());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debounce_collapses_a_change_then_fires_once_quiet() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
