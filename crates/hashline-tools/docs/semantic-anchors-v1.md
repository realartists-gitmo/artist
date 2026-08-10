# TECA semantic anchors

Logical-line occurrence identities are built by `artist-ast` from canonical line bytes, label-free CST kind/role ancestry, and bidirectional equivalent-sibling rank. The exact identity bytes are passed directly to the published `teca` crate—without normalization, trimming, case folding, or prehashing.

Artist renders the shortest TECA structural-atom prefix unique among the live occurrences in the resource. The wire representation is `#` followed by TECA's boundary-preserving length-and-hex atom format. It is opaque to callers and deterministic across processes; no allocator or actor-local address state exists.
