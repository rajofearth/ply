# Hand-roll the explorer shell; keep gpui-component only for text input

Supersedes most of [0001](0001-gpui-component.md).

The UI is now specified by a reference design with exact tokens: a zero-hue
OKLCH neutral scale, `radius: 0`, a native UI font stack, and lucide icons. Two
things made gpui-component's widgets a poor fit for that.

**Icons.** `gpui-component-assets` ships a curated ~99-icon subset of lucide.
Nine glyphs the design needs — `image`, `music`, `video`, `layout-grid`, `list`,
`home`, `square`, `usb`, `file-text` — are absent, and lucide's `x` is renamed
`close`. Reaching for near-matches is what made an earlier attempt drift off the
design. We vendor the lucide SVGs we use into `assets/icons/` and serve them
from our own `AssetSource` only — no fallback into gpui-component's icon pack.
A unit test loads every `Ico` variant, so a missing file fails the build rather
than rendering an invisible glyph. Dropping that pack (and a size-minded
release profile: thin LTO, `opt-level = "s"`, strip) keeps the binary leaner;
GPUI still dominates size and RAM.

**Colour.** GPUI has no OKLCH constructor. Tokens are converted to sRGB once, at
authoring time, and `src/theme.rs` records the OKLCH source next to each value
so the two can be checked against each other. Our `Palette` is a plain struct,
not a `gpui_component::Theme`, because the design's slots (`chartBar`,
`chartBarTrack`, `selectStrong`) do not map onto that theme's semantic set.

Rows, cards, the sidebar tree, the context menu, and the Properties dialog are
plain `div()`s. They are mostly layout and hover state, so the widget library
bought us little while constraining the styling. `Input` is the exception: a
correct text field is genuinely hard, so the filter box and inline rename use
`gpui_component::input::{Input, InputState}`.

GPUI has no letter-spacing, so the design's `0.08em` section labels rely on
uppercasing and size alone. GPUI's `grid_cols` takes a fixed column count, so
the design's `repeat(auto-fill, minmax(…))` card and icon grids are flex-wrap.
