# Ply engineering philosophy

Software stays small and fast on purpose, never by accident. These are the beliefs behind that purpose.

## Do not link it

The smallest binary is the one you never link. Every dependency is a standing cost paid on every launch, in bytes and in startup time, whether its code runs or not.

The suckless tradition judges skill by lines removed. Apply the same test to the dependency list. A crate that serves one call site has to justify itself against the local code that would replace it.

## Let the platform do the hard work

Whatever the machine underneath already provides, call it instead of bundling it. Decoders, shapers, thumbnail caches, the compositor. They ship with the OS, they update with the OS, and they cost this binary nothing.

This is mechanical sympathy in the original sense. Work in harmony with the layer below you rather than fighting it. It does not mean the platform is always faster. It means you check before you carry your own copy.

## Do work on demand

Nothing loads, scans, or warms up before something on screen needs it. Enumerate the folder in view, not the drive. Expand the branch the user opened, not the tree. Fetch the icon when the row paints.

Two borrowed ideas share this one. YAGNI says do not build the speculative feature. Demand paging says do not touch the page before the fault. One governs what gets built, the other governs when it runs.

## Never redo work

If the inputs did not change, the output is already known. Fingerprint the listing and skip the reload. Key the cache and skip the extraction. Render only the rows in the window plus a little overscan.

DRY names this, with one honest exception its own authors allow. A cache is a deliberate local duplication, and it stays honest only while the key stays exact. The moment two paths can disagree about freshness, the cache becomes the bug.

## Measure first

No optimization without a number. Profile the binary by section, weigh each dependency with the tree, time the hot path on real data. Then decide.

Knuth, in full rather than the bumper sticker: forget small efficiencies 97 percent of the time, because guesses about hot spots fail. Then spend everything on the measured 3 percent, where even a loop win is worth taking. Budgets are how this survives contact with pull requests. A ceiling that fails the build beats a paragraph that asks nicely.

## Small core, thin shells

Push complexity down behind small interfaces and keep the core portable. Platform code lives behind a trait, and every unsupported target gets a stub that compiles, so the core never learns the name of an OS API.

Gall's law earns its place with the qualifier intact. A working complex system evolves from a working simple one, and a simple one may or may not work. So the core ships simple first and the shells arrive later, one platform at a time. Ousterhout's deep modules say the same from the other side. Small surface, real work hidden underneath.

## Prefer boring local code

Simple is not the same as easy. Hickey's distinction holds up. Simple means unentangled, one fold, nothing hidden. Easy means near at hand, familiar to the reader. A framework call is easy. A hundred lines of plain local code is simple. Prefer simple.

The boring technology argument sets the price. Every novel dependency spends an innovation token and brings failure modes nobody in the room has debugged yet. Spend tokens where the product lives, never on plumbing.

## Pay for use only

Stroustrup's zero-overhead principle, stated straight: what you do not use, you do not pay for. Features compile per target, codecs link per format, backends ship per OS. Anything unconditional is a candidate for a flag or a fork.

Read the caveat with the quote. It promises no overhead versus hand-written code, not zero cost. The binary still has a floor, set by the runtime, the renderer, and the standard library. Know the floor so nobody chases bytes below it.
