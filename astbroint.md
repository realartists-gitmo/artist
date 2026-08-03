Map---read hook

Digest---

Show

Implements

Surface

Deps---appended on first read to a file

Reverse deps---appended on first read to a file

Cycles

Graph

Callers---subsumed by impact

Callees---subsumed by impact

Trace

Impact---appended on first edit to a symbol. Will also have a tool for direct symbol query

Context

Run

Squeeze---Parked.

Index/search/find---overlaps with mem

Your instinct to check was right — the read hook is bypassable.

There's a passing test proving it: anchor_from_outline_drives_edit_without_an_intervening_read. map issues anchors, so the model can go map → edit and never call read. A hook on the read tool silently misses that path.

The infallible hook point is anchor issuance, not read. Every path that lets the model address a line goes through reconcile_path_anchors — six call sites, covering read, map, edit, write and drift. You cannot name a line in a file without it having run. Hooking there is infallible by construction; hooking read is a convention.

Same shape for impact. Content commits through two coordinator entry points, write_file and edit_file, and run --apply goes through write_file. Hooking edit alone misses whole-file writes and every structural rewrite.

So: deps/reverse-deps on first anchor issuance for a path; impact on commit in the coordinator, not on the edit tool.

The rest

Two more have genuinely infallible homes:

surface — on first anchor issuance for a module root (lib.rs, mod.rs, __init__.py). That's precisely the file where a raw read is least informative: you get a wall of pub use lines instead of what's actually exported. Same trigger as deps, filtered by filename.

map/digest — on anchor issuance for a file over some line threshold, shape instead of (or before) content. This is exactly what ast-bro's own hook command did with --min-lines 200, which we deleted as plumbing. Infallible, but it changes what read returns, so it's a behavioural call rather than a free addition.
