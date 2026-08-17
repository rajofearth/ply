# Quick Look renders contents; Properties stays metadata

CONTEXT originally said Ply never renders file contents: Properties replaced a
preview pane. That rule is retired for the spacebar overlay only.

Quick Look is a transient overlay on the selected Entry. It is not a pane, not
Properties, and not Open. Images and text render in-process. PDF, Office, and
media use a free thumbnailer or OS handler when one is present (`pdftoppm`,
`ffmpeg`, and platform thumbnail APIs); otherwise the overlay shows kind, size,
and a prompt to Open.

RAM stays under the budget gate in `src/budget.rs`: text is capped, images go
through GPUI's loader, generated thumbnails land in a temp dir and die with the
overlay.
