**Ply**

if a task can be delegated to the subagent, whether it's research, exploration, or writing code, please use subagents. Use multiple if necessary.
always talk without slop(`unslop`).

read on the matching branch. names, avoids, and UI vocabulary live in `CONTEXT.md`. the listing model and why there's no worktree is in `docs/adr/0002-listing-not-worktree.md`. the hand-rolled shell and input-only gpui-component decision is in `docs/adr/0003-hand-rolled-explorer-shell.md`. 
settled base, PathCaps, and listing/UI/volume efficiency is in `docs/adr/0004-settled-base-and-efficiency.md`. build/test/run commands are in `README.md`. binary and RAM budgets plus the size gate are in `src/budget.rs`.

run `cargo test budgets_report -- --nocapture` and obey the printed gate.

gotcha: a running Ply shows about 300 MB working set, but that's GPU shared memory and doesn't count. only non-GPU working set counts toward the RAM ceiling.

this project uses GPUI (Zed's UI framework), not to be confused with general GUI/GPU terms.

subagents must ground themselves in the actual source/examples below before writing GPUI code; do not rely on memorized API shapes.

**reference sources (fetch before implementing)**

- repo: https://github.com/zed-industries/zed (GPUI lives in `crates/gpui/`)
- concepts doc: `crates/gpui/docs/contexts.md`
- crate README: `crates/gpui/README.md`
- working examples (primary source of truth): `crates/gpui/examples/`
  - `hello_world.rs` — minimal app skeleton
  - `gif_viewer.rs` — image loading/rendering
  - `scrollable.rs`, `uniform_list.rs` — scrolling lists
  - `grid_layout.rs` — grid layouts
  - `input.rs` — text input handling
- component library (higher-level, better docs): https://longbridge.github.io/gpui-component/llms.txt
  - getting started: https://longbridge.github.io/gpui-component/docs/getting-started.md
  - design guidelines: https://longbridge.github.io/gpui-component/docs/design-guides.md
  - coding guidelines: https://longbridge.github.io/gpui-component/docs/coding-guides.md
  - icons & assets: https://longbridge.github.io/gpui-component/docs/assets.md
  - `story` crate in that repo — full working gallery app of all components, use as a reference implementation
- architecture overview (auto-generated, cross-check against source): https://deepwiki.com/zed-industries/zed
- real shipped GPUI apps for pattern reference: https://github.com/zed-industries/awesome-gpui

**subagent instructions**

- before writing any GPUI code, read the relevant example file(s) above, not just this summary.
- prefer gpui-component over raw GPUI primitives unless the task specifically requires low-level control.
- if an API doesn't match what's in the examples, trust the examples over memory.
- subagents and you too are required to read `unslop` n `blast radius` skiils if not available then https://github.com/cursor/plugins/tree/main/pstack/skills/.

**change impact discipline**

before modifying any function, module, or shared resource:

1. trace its `blast radius`: what calls it, what it calls, what shares state or config with it.
2. state any invariants the surrounding code relies on (data always sorted, auth always checked first, cache always invalidated on write, etc.) and confirm the change doesn't break them.
3. note anything at the edges that's affected: security boundaries, memory/perf-sensitive paths, or anywhere untrusted input touches this code.
4. explicitly consider performance impact, code quality, and security. ask whether a better, cleaner, faster, or more efficient approach already exists (library, pattern, existing code elsewhere) or whether an entirely different way of solving it would be superior. use subagents for unbiased research and alternative ideas when helpful.
5. if the blast radius, invariant list, or the set of quality/security/performance concerns is non-trivial, say so explicitly before writing the diff.

after the change:

1. re-evaluate performance impact, code quality, and security of what was just written.
2. look for further improvements: cleaner structure, faster paths, better efficiency, or opportunities to slice/refactor the code.
3. check whether the new logic duplicates (or is weaker than) something already present elsewhere in the codebase. if a better version exists, update callers to use it and remove the duplication so the codebase stays clean.
4. confirm the change still respects the original blast radius and invariants.

this applies to subagents and to you directly; no exceptions for "small" changes.

parent plans and integrates; sub-agents dig. Dispatch for parallel, high-context, or specialist work: architecture, performance, research, docs, bug/security/spec review. grill one question at a time.

prefer tickets (epic plus children) for non-trivial work; close them as you finish. Parallelize independent digs, but keep one writer per hot file. Measure before every performance patch. Plan, get approval, then implement on cross-cutting refactors, unless the user already said go.

MTP feature growth stays deferred until the user reopens it (ADR 0004). Expand scope only when asked.
