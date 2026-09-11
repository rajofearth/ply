//! Headless listing bench (D10): 10k files in a temp dir, timing the real
//! `listing.rs` code with no window and no new dependencies.
//!
//! Ply is a binary crate, so the real module is included by path. The two
//! modules its folder routing touches (Recycle Bin, MTP) are stubbed to
//! "not here", which is exactly what a temp-dir listing hits, so every timed
//! call below runs the shipping code.
//!
//! Run with `cargo test --test bench_listing -- --nocapture` to see the
//! numbers. Ceilings are deliberately loose (slow CI): they catch algorithmic
//! regressions (per-comparison allocation, per-keystroke realloc), not noise.
//! The old-algorithm references time the pre-D10 approach on the same data in
//! the same run, so the before/after comparison has no cross-run skew.

// The `listing.rs` copy pulls in icon/format helpers this bench never calls.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

mod icons {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Ico {
        Folder,
        Image,
        Video,
        Music,
        FileText,
        File,
    }
}

mod recycle_bin {
    use std::path::Path;

    pub fn is_recycle_bin(_path: &Path) -> bool {
        false
    }

    pub fn list() -> anyhow::Result<Vec<super::listing::Entry>> {
        Ok(Vec::new())
    }
}

mod mtp {
    use std::path::Path;

    pub fn is_mtp(_path: &Path) -> bool {
        false
    }

    pub fn list(_path: &Path) -> anyhow::Result<Vec<super::listing::Entry>> {
        Ok(Vec::new())
    }
}

#[path = "../src/listing.rs"]
mod listing;

use listing::{Snapshot, SortKey, filter_indices_into, list_sorted};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Timing ceilings under a full `cargo test` are load-sensitive, so the tight
/// budget is opt-in. The loose ceiling always runs to catch a hang or an
/// accidental O(n^2). Set `PLY_BENCH_STRICT=1` on an idle machine to enforce
/// the real regression budget. Correctness assertions below are always on.
fn assert_within(label: &str, value: f64, loose: f64, strict: f64) {
    assert!(
        value < loose,
        "{label} took {value:.1} ms, loose ceiling {loose:.0} ms (hang or blowup?)"
    );
    if std::env::var_os("PLY_BENCH_STRICT").is_some() {
        assert!(
            value < strict,
            "{label} took {value:.1} ms, strict ceiling {strict:.0} ms"
        );
    }
}

/// The pre-D10 filter: lower every name on every keystroke.
fn old_filter(entries: &[listing::Entry], needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return (0..entries.len()).collect();
    }
    let lowered = needle.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name.to_lowercase().contains(&lowered))
        .map(|(i, _)| i)
        .collect()
}

struct Rec {
    name: String,
    dir: bool,
    kind: &'static str,
    size: u64,
    modified: Option<SystemTime>,
}

/// The pre-D10 sort: lower (allocating) inside every comparison.
fn old_sort_names(entries: &[listing::Entry], key: SortKey) -> Vec<String> {
    let mut recs: Vec<Rec> = entries
        .iter()
        .map(|e| Rec {
            name: e.name.clone(),
            dir: e.is_directory(),
            kind: listing::kind_label(e),
            size: e.size,
            modified: e.modified,
        })
        .collect();
    recs.sort_by(|a, b| {
        b.dir.cmp(&a.dir).then_with(|| match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Modified => b.modified.cmp(&a.modified),
            SortKey::Kind => a
                .kind
                .cmp(b.kind)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            SortKey::Size => b
                .size
                .cmp(&a.size)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        })
    });
    recs.into_iter().map(|r| r.name).collect()
}

fn new_sorted_names(snap: &Snapshot) -> Vec<String> {
    snap.entries.iter().map(|e| e.name.clone()).collect()
}

struct TempDir(PathBuf);

impl TempDir {
    fn fresh(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ply-bench-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bench_listing_10k() {
    let tmp = TempDir::fresh("10k");
    let dir = &tmp.0;
    // 9_800 ASCII files with varied extensions and sizes, 200 non-ASCII
    // files (both filter paths get exercised), 200 subdirectories.
    let exts = ["txt", "md", "jpg", "mp4", "zip", "log"];
    for i in 0..9_800 {
        let name = format!("file_{i:05}.{}", exts[i % exts.len()]);
        let body = vec![b'x' + (i % 26) as u8; (i * 37) % 251];
        std::fs::write(dir.join(name), body).unwrap();
    }
    for i in 0..100 {
        std::fs::write(dir.join(format!("café_{i:03}.txt")), b"e").unwrap();
        std::fs::write(dir.join(format!("界_{i:03}.md")), b"cjk").unwrap();
    }
    for i in 0..200 {
        std::fs::create_dir(dir.join(format!("dir_{i:03}"))).unwrap();
    }
    let total = 9_800 + 200 + 200;
    eprintln!(
        "--- bench_listing_10k: {total} files + 200 dirs in {}",
        dir.display()
    );

    // Phase 1: cold list + default sort.
    let t = Instant::now();
    let mut snap = list_sorted(dir, SortKey::default()).unwrap();
    let list_ms = ms(t.elapsed());
    assert_eq!(snap.entries.len(), total);
    eprintln!("list_sorted (default key): {list_ms:.1} ms for {total} entries");

    // Phase 2: resort across every key, checking order against a fresh sort
    // where the order is total (Name/Kind/Size break ties by name).
    for key in [
        SortKey::Name,
        SortKey::Modified,
        SortKey::Kind,
        SortKey::Size,
    ] {
        let t = Instant::now();
        snap.resort(key);
        let resort_ms = ms(t.elapsed());
        let fresh = Snapshot::sorted(
            list_sorted(dir, SortKey::default())
                .unwrap()
                .entries
                .iter()
                .cloned()
                .collect(),
            key,
        );
        if key == SortKey::Modified {
            // mtimes tie within the creation burst and stable sort keeps
            // input order, so only the dirs-first partition is asserted.
            let names = new_sorted_names(&snap);
            let first_file = names.iter().position(|n| !dir.join(n).is_dir());
            let last_dir = names.iter().rposition(|n| dir.join(n).is_dir());
            assert!(
                last_dir < first_file,
                "dirs must still sort first under {key:?}"
            );
        } else {
            assert_eq!(
                new_sorted_names(&snap),
                new_sorted_names(&fresh),
                "resort order must equal fresh sort for {key:?}"
            );
        }
        eprintln!("resort {key:?}: {resort_ms:.1} ms");
        assert_within(&format!("resort {key:?}"), resort_ms, 3000.0, 100.0);
    }

    // Phase 3: old-vs-new sort on identical input (Name has no ties, so the
    // orders must match exactly). One sort each side.
    let t = Instant::now();
    let old_order = old_sort_names(&snap.entries, SortKey::Name);
    let old_ms = ms(t.elapsed());
    let mut resorted = Snapshot::sorted(snap.entries.iter().cloned().collect(), SortKey::Modified);
    let t = Instant::now();
    resorted.resort(SortKey::Name);
    let new_ms = ms(t.elapsed());
    assert_eq!(new_sorted_names(&resorted), old_order);
    eprintln!(
        "sort Name: old per-compare lowering {old_ms:.1} ms, new precomputed keys {new_ms:.1} ms"
    );

    // Phase 4: keystroke simulation, one shared scratch vec like the app.
    let mut scratch = Vec::new();
    let mut worst = 0.0f64;
    let mut typed = String::new();
    for ch in "file_0199".chars() {
        typed.push(ch);
        let t = Instant::now();
        filter_indices_into(&snap.entries, &typed, &mut scratch);
        let dt = ms(t.elapsed());
        worst = worst.max(dt);
        assert_eq!(
            scratch,
            old_filter(&snap.entries, &typed),
            "needle {typed:?}"
        );
    }
    let t = Instant::now();
    let old_hits = old_filter(&snap.entries, &typed);
    let old_filter_ms = ms(t.elapsed());
    let t = Instant::now();
    filter_indices_into(&snap.entries, &typed, &mut scratch);
    let new_filter_ms = ms(t.elapsed());
    assert_eq!(scratch, old_hits);
    eprintln!(
        "filter typing \"{typed}\" ({} keystrokes, scratch reused): worst keystroke {worst:.2} ms",
        typed.len()
    );
    eprintln!(
        "filter final: old per-entry lowering {old_filter_ms:.2} ms, new zero-alloc {new_filter_ms:.2} ms"
    );
    assert_within("slowest keystroke", worst, 3000.0, 100.0);

    // Loose overall ceiling: listing is FS-bound (stats + attrs per file).
    assert_within("list_sorted", list_ms, 10000.0, 3000.0);
    eprintln!(
        "PASS ceilings: loose always (list < 10000 ms, resort/filter < 3000 ms), strict with PLY_BENCH_STRICT=1"
    );
}
