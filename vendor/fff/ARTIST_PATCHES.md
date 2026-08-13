# Artist patches to FFF

Artist vendors FFF at the commit recorded in `UPSTREAM` and keeps FFF behind
`artist-kernel::SearchService`.

## `grep_raw`

Artist owns the pattern grammar (`lit:`, `re:`, and `fz:`). FFF's normal
`grep` entry point accepts and interprets its own query language, including a
literal fallback for inferred constraints. That is not an acceptable semantic
boundary for Artist. The vendor therefore exposes `FilePicker::grep_raw`,
which constructs a plain text query and calls the parsed search engine without
query parsing or fallback.

Artist supplies the mode and all pagination/time limits explicitly. It reads
the file before and after the search and rejects a changed file, so matches
are never converted into anchors from a different byte snapshot.

## `fuzzy_line_matches`

Artist's `grep(Text)` and CR-only fallback searches have no `FilePicker`.
The vendor therefore exposes a pure boolean line matcher using the same
scoring and acceptance checks as FFF fuzzy grep. It carries no filesystem,
frecency, query-history, or result-type state.

## `fuzzy_match_score`

Artist `find` needs deterministic fuzzy path ranking for filesystem and
virtual candidates. The vendor exposes the FFF/neo-frizbee score as a pure
candidate function, with smart casing and no picker, frecency, or
query-history state. Artist applies the score-descending, URI-lexical
tie-break.
