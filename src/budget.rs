//! RAM and binary-size gates. Run with:
//! `cargo test budgets_report -- --nocapture`
//!
//! Numbers are ceilings, not targets. Measure before tightening or before
//! landing a "performance" patch that does not move them.

use std::path::PathBuf;

/// Debug `ply` binary. GPUI debug info is large; this is a backstop against
/// accidental asset bloat, not a shipping number.
pub const MAX_DEBUG_BIN_MIB: u64 = 450;

/// Release `ply` binary. Raise only with a measured reason in the PR.
pub const MAX_RELEASE_BIN_MIB: u64 = 90;

/// Resident set of this test process. The explorer window is measured
/// separately when a human (or the GUI exercise) runs `cargo run`.
pub const MAX_TEST_RSS_MIB: u64 = 256;

pub fn current_rss_mib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in text.lines() {
            let Some(kb) = line.strip_prefix("VmRSS:") else {
                continue;
            };
            let kb = kb
                .trim()
                .trim_end_matches(" kB")
                .trim()
                .parse::<u64>()
                .ok()?;
            return Some(kb / 1024);
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub fn ply_binaries() -> Vec<(PathBuf, &'static str)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let mut out = Vec::new();
    let debug = root.join("debug").join(bin_name());
    let release = root.join("release").join(bin_name());
    if debug.is_file() {
        out.push((debug, "debug"));
    }
    if release.is_file() {
        out.push((release, "release"));
    }
    out
}

fn bin_name() -> &'static str {
    if cfg!(windows) { "ply.exe" } else { "ply" }
}

pub fn file_mib(path: &std::path::Path) -> Option<u64> {
    let bytes = std::fs::metadata(path).ok()?.len();
    Some(bytes.div_ceil(1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_report() {
        let rss = current_rss_mib();
        match rss {
            Some(mib) => {
                println!("test_rss_mib={mib} cap={MAX_TEST_RSS_MIB}");
                assert!(
                    mib <= MAX_TEST_RSS_MIB,
                    "test RSS {mib} MiB exceeds {MAX_TEST_RSS_MIB} MiB"
                );
            }
            None => println!("test_rss_mib=unmeasured"),
        }

        let bins = ply_binaries();
        if bins.is_empty() {
            println!("ply_binary=absent (run cargo build to include size in the gate)");
        }
        for (path, profile) in bins {
            let Some(mib) = file_mib(&path) else { continue };
            let cap = match profile {
                "release" => MAX_RELEASE_BIN_MIB,
                _ => MAX_DEBUG_BIN_MIB,
            };
            println!(
                "ply_binary={} profile={profile} size_mib={mib} cap={cap}",
                path.display()
            );
            assert!(
                mib <= cap,
                "{} ({profile}) is {mib} MiB; cap is {cap} MiB",
                path.display()
            );
        }

        if let Ok(exe) = std::env::current_exe()
            && let Some(mib) = file_mib(&exe)
        {
            println!("test_exe={} size_mib={mib}", exe.display());
        }
    }
}
