//! Product budgets for agents and CI feedback.
//!
//! `cargo test` always prints the report below. When a release binary exists,
//! its size is gated at 10 MiB so agents see a hard fail, not a vibe.
//!
//! RAM is opt-in and report-only by default. `PLY_BUDGET_MEASURE_EXE=1` is the
//! only trigger that launches the release exe. It samples **non-GPU working
//! set**: process `WorkingSet64` minus the sum of that process's
//! `GPU Process Memory(*)\Shared Usage` perf counters. `PrivateMemorySize64`
//! is not used because it tracks GPU-shared allocations and false-fails the
//! policy ceiling. Set `PLY_BUDGET_REQUIRE_RAM=1` to assert the sampled value.

/// Soft ceiling for release `ply` on disk (ADR 0004 / AGENTS.md).
#[allow(dead_code)]
pub const MAX_RELEASE_BYTES: u64 = 10 * 1024 * 1024;

/// Non-GPU working-set ceiling: working set minus GPU-shared bytes. GPU shared
/// memory is ignored by product policy.
#[allow(dead_code)]
pub const MAX_NON_GPU_WORKING_SET_BYTES: u64 = 100 * 1024 * 1024;

#[cfg(test)]
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
        measure_working_set_gate(&path);

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

    fn env_flag(name: &str) -> bool {
        std::env::var(name).as_deref() == Ok("1")
    }

    fn report_measurement_failure(required: bool, reason: &str) {
        eprintln!(
            "RAM measure:            ({reason}) {}",
            if required { "FAIL" } else { "SKIP" }
        );
        assert!(!required, "required RAM measurement failed: {reason}");
    }

    fn non_gpu_working_set(working_set: u64, gpu_shared: u64) -> Option<u64> {
        working_set.checked_sub(gpu_shared)
    }

    /// Launches the release exe, reports its non-GPU working set, and closes it.
    /// Print-only unless `PLY_BUDGET_REQUIRE_RAM=1`; only
    /// `PLY_BUDGET_MEASURE_EXE=1` launches the exe, so normal tests and CI do not.
    fn measure_working_set_gate(path: &std::path::Path) {
        let required = env_flag("PLY_BUDGET_REQUIRE_RAM");
        if !cfg!(windows) {
            return;
        }
        if !env_flag("PLY_BUDGET_MEASURE_EXE") {
            if required {
                report_measurement_failure(required, "PLY_BUDGET_MEASURE_EXE is not set");
            }
            return;
        }
        let Ok(mut child) = std::process::Command::new(path)
            .env("PLY_BUDGET_MEASURE", "1")
            .spawn()
        else {
            report_measurement_failure(required, "release exe launch failed");
            return;
        };
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_secs(10));
        // Working set includes GPU-shared memory. The GPU perf counter splits
        // that out; print-only if the driver does not expose the counter.
        let script = r#"
$procId = __PID__
$p = Get-Process -Id $procId -ErrorAction Stop
$shared = $null
try {
  $samples = (Get-Counter "\GPU Process Memory(pid_${procId}_*)\Shared Usage" -ErrorAction Stop).CounterSamples
  if ($samples.Count -gt 0) {
    $shared = [int64](($samples | Measure-Object -Property CookedValue -Sum).Sum)
  }
} catch {}
if ($null -eq $shared) { Write-Output "$($p.WorkingSet64) NA" }
else { Write-Output "$($p.WorkingSet64) $shared" }
"#
        .replace("__PID__", &pid.to_string());
        let out = match std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
        {
            Ok(out) => out,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                report_measurement_failure(required, "PowerShell could not be started");
                return;
            }
        };
        let _ = child.kill();
        let _ = child.wait();
        if !out.status.success() {
            report_measurement_failure(required, "PowerShell measurement failed");
            return;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut fields = text.split_whitespace();
        let Some(working_set) = fields.next().and_then(|word| word.parse::<u64>().ok()) else {
            report_measurement_failure(required, "PowerShell output was not parseable");
            return;
        };
        let gpu_shared = match fields.next() {
            Some("NA") => None,
            Some(word) => match word.parse::<u64>() {
                Ok(bytes) => Some(bytes),
                Err(_) => {
                    report_measurement_failure(required, "PowerShell output was not parseable");
                    return;
                }
            },
            None => {
                report_measurement_failure(required, "PowerShell output was not parseable");
                return;
            }
        };
        let Some(gpu_shared) = gpu_shared else {
            eprintln!("RAM non-GPU working set:   --");
            eprintln!(
                "RAM working set (total):   {:>6.2} MiB  (includes GPU shared; not gated)",
                working_set as f64 / (1024.0 * 1024.0)
            );
            report_measurement_failure(required, "GPU-shared counter unavailable");
            return;
        };
        let Some(non_gpu) = non_gpu_working_set(working_set, gpu_shared) else {
            report_measurement_failure(
                required,
                "GPU-shared memory exceeded the total working set",
            );
            return;
        };
        let mib = non_gpu as f64 / (1024.0 * 1024.0);
        let ok = non_gpu <= MAX_NON_GPU_WORKING_SET_BYTES;
        eprintln!(
            "RAM working set (total):   {:>6.2} MiB",
            working_set as f64 / (1024.0 * 1024.0)
        );
        eprintln!(
            "RAM GPU shared (excluded): {:>6.2} MiB",
            gpu_shared as f64 / (1024.0 * 1024.0)
        );
        eprintln!(
            "RAM non-GPU working set:   {mib:>6.2} MiB  {}",
            if ok { "PASS" } else { "FAIL" }
        );
        if required {
            assert!(
                ok,
                "non-GPU working set is {mib:.2} MiB ({non_gpu} bytes); budget is {} bytes \
                 (100 MiB). Find what has grown or justify raising AGENTS.md / budget.rs together.",
                MAX_NON_GPU_WORKING_SET_BYTES
            );
        }
    }

    #[test]
    fn impossible_ram_measurement_is_rejected() {
        assert_eq!(non_gpu_working_set(100, 101), None);
    }

    #[test]
    #[should_panic(expected = "required RAM measurement failed")]
    fn required_ram_measurement_failure_panics() {
        report_measurement_failure(true, "test failure");
    }
}
