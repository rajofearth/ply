//! Product budgets for agents and CI feedback.
//!
//! `cargo test` always prints the report below. When a release binary exists,
//! its size is gated at 10 MiB so agents see a hard fail, not a vibe.

/// Soft ceiling for release `ply` on disk (ADR 0004 / AGENTS.md).
pub const MAX_RELEASE_BYTES: u64 = 10 * 1024 * 1024;

/// Non-GPU working-set ceiling. GPU shared memory is ignored by product policy.
pub const MAX_NON_GPU_WORKING_SET_BYTES: u64 = 100 * 1024 * 1024;

fn release_bin_path() -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("release");
    if cfg!(windows) {
        path.push("ply.exe");
    } else {
        path.push("ply");
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_report_and_release_size_gate() {
        let path = release_bin_path();
        eprintln!("--- ply budgets (AGENTS.md) ---");
        eprintln!(
            "release binary max:     {:>6.2} MiB",
            MAX_RELEASE_BYTES as f64 / (1024.0 * 1024.0)
        );
        eprintln!(
            "non-GPU working set max:{:>6.2} MiB  (ignore GPU shared)",
            MAX_NON_GPU_WORKING_SET_BYTES as f64 / (1024.0 * 1024.0)
        );
        eprintln!("idle CPU:               near-zero on Home");
        eprintln!("path checked:           {}", path.display());

        match std::fs::metadata(&path) {
            Ok(meta) => {
                let bytes = meta.len();
                let mib = bytes as f64 / (1024.0 * 1024.0);
                let ok = bytes <= MAX_RELEASE_BYTES;
                eprintln!(
                    "release size:           {mib:>6.2} MiB  {}",
                    if ok { "PASS" } else { "FAIL" }
                );
                assert!(
                    ok,
                    "release binary is {mib:.2} MiB ({bytes} bytes); budget is {} bytes (10 MiB). \
                     Shrink deps/profile or justify raising AGENTS.md / budget.rs together.",
                    MAX_RELEASE_BYTES
                );
            }
            Err(_) => {
                eprintln!(
                    "release size:           (missing)  SKIP gate — run `cargo build --release` \
                     before claiming a size win; set PLY_BUDGET_REQUIRE_RELEASE=1 to fail here"
                );
                if std::env::var_os("PLY_BUDGET_REQUIRE_RELEASE").is_some() {
                    panic!(
                        "PLY_BUDGET_REQUIRE_RELEASE is set but {} was not found",
                        path.display()
                    );
                }
            }
        }
        eprintln!("-------------------------------");
    }
}