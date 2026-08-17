# Ply

## Read on the matching branch

| Branch | Source of truth |
| --- | --- |
| Names, Avoids, UI vocabulary | [CONTEXT.md](CONTEXT.md) |
| Listing model, why no worktree | [docs/adr/0002-listing-not-worktree.md](docs/adr/0002-listing-not-worktree.md) |
| Hand-rolled shell, Input-only gpui-component | [docs/adr/0003-hand-rolled-explorer-shell.md](docs/adr/0003-hand-rolled-explorer-shell.md) |
| Settled base, PathCaps, listing/UI/volume efficiency | [docs/adr/0004-settled-base-and-efficiency.md](docs/adr/0004-settled-base-and-efficiency.md) |
| Build / test / run commands | [README.md](README.md#commands) |
| Binary + RAM budgets and the size gate | [src/budget.rs](src/budget.rs) |

## Budgets

Run `cargo test budgets_report -- --nocapture` and obey the printed gate.
Gotcha: a running Ply shows ~300 MB working set, but that is GPU shared memory
and does not count — only non-GPU working set counts toward the RAM ceiling.

## Orchestration

Parent plans and integrates; sub-agents dig. Dispatch for parallel,
high-context, or specialist work — architecture, performance, research, docs,
bug/security/spec review. Grill one question at a time.

Prefer tickets (epic + children) for non-trivial work; close them as you finish.
Parallelize independent digs, but keep one writer per hot file. Measure before
every performance patch. Plan → approve → implement on cross-cutting refactors
unless the user already said go.

MTP feature growth stays deferred until the user reopens it (ADR 0004). Expand
scope only when asked.
