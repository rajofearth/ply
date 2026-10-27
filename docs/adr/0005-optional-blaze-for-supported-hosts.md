# Use Blaze as an optional development accelerator

## Status

Proposed. Blaze is optional on hosts it supports. Cargo remains the required
build tool for Ply.

## Context

Ply is developed and shipped first on Windows ARM64
(`aarch64-pc-windows-msvc`). Blaze is an incremental Rust build daemon that can
recompile affected functions instead of rebuilding the affected crates with
Cargo.

The current Blaze platform support does not include native Windows ARM64. Its
Windows implementation targets x86-64 MSVC. Linux ARM64 and macOS ARM64 are
supported development platforms.

We measured the same Ply source in WSL2 Debian on native ARM64 Linux, using
separate Cargo and Blaze targets on the WSL ext4 filesystem. The Blaze daemon
served the measured builds rather than falling back to Cargo.

| Workload | Cargo | Blaze |
| --- | ---: | ---: |
| Warm no-op build | 3.51 s | 0.06 s |
| Body-only edit build | 27.47 s | 2.13 s |

The body-edit result is about 12.9x faster with Blaze. Its target and daemon
data used about 2.6 GiB and 259 MiB of cache, compared with about 2.0 GiB for
Cargo's target directory. Cold-build timings were not used because the two
initial runs contended for Cargo's package-cache lock.

Ply's current CI keeps the release build on Windows and runs check-only jobs
on macOS and Linux in `.github/workflows/size-delta.yml`. Those jobs are also
the authoritative cross-platform checks for the repository.

## Decision

- Keep Cargo as the required tool for local checks, pull requests, releases,
  and production builds.
- Allow developers on supported macOS and Linux ARM64 hosts to use Blaze as an
  opt-in local edit-loop accelerator.
- Do not add Blaze to `Cargo.toml`, vendor its compiler, fork it into Ply, or
  make CI depend on its daemon.
- If we later publish a convenience setup for Blaze, pin a known external
  release in developer documentation or a setup script. Do not turn it into a
  Ply runtime module or a second compiler dependency.
- Do not use a Linux or WSL Blaze build as evidence that the native Windows
  ARM64 application builds or runs. It exercises Linux cfgs, Linux GPUI
  integration, and a Linux executable.
- Revisit native Windows ARM64 support when Blaze publishes and tests an
  `aarch64-pc-windows-msvc` implementation. Cargo remains the final check for
  the Windows application until then.

## Alternatives considered

### Port Blaze inside Ply

This would make Ply responsible for a compiler, rust-analyzer-derived front
end, code generator, PE linker/splicer, Windows ARM64 ABI and SEH support. It
would add a large maintenance boundary to a file explorer and would still
need Cargo for release validation. The measured speedup does not justify that
scope today.

### Maintain a Ply fork or vendored Blaze module

A pinned external binary is easy to replace and keeps the compiler separate. A
fork or vendored module would require merging upstream compiler and toolchain
changes, maintaining Windows support, and debugging mismatches with Ply's GPUI
dependencies. This is deferred until upstream support leaves a small,
well-defined port.

### Use Blaze in GitHub Actions

The first build still loads the daemon and uses more memory and disk than
Cargo. Shared hosted runners would pay that startup cost on every job. CI also
needs the native Windows release build, which Blaze cannot currently serve on
Windows ARM64. A future dedicated ARM64 Linux benchmark job may be useful for
tracking edit-loop performance, but it should not replace the existing checks.

### Build the Windows application under WSL

WSL is useful for measuring Blaze's Linux path. It cannot validate Ply's
Windows-only dependencies, Windows resources, shell integration, MTP support,
or native GPUI runtime.

## Reconsider when

Reopen this decision if one of these becomes true:

- Blaze supports and tests native `aarch64-pc-windows-msvc` builds, including
  COFF/PE splicing and Windows unwind behavior.
- Ply's native edit loop becomes a measured bottleneck large enough to justify
  maintaining a compiler integration.
- Blaze exposes a stable, versioned interface suitable for an optional
  developer tool without pinning Ply's release toolchain.

## References

- [Blaze repository](https://github.com/indexable-inc/blaze)
- [Blaze usage](https://github.com/indexable-inc/blaze/blob/main/docs/usage.md)
- [Blaze platform support](https://github.com/indexable-inc/blaze/blob/main/docs/support.md)
- [Blaze platform implementation](https://github.com/indexable-inc/blaze/blob/main/docs/platforms.md)
- [Blaze benchmarks](https://github.com/indexable-inc/blaze/blob/main/docs/benchmarks.md)
- [Ply size and cross-platform workflow](../../.github/workflows/size-delta.yml)
