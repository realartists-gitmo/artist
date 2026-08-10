# Verification log

Active semantic pre-label contract: `muse-semantic-label-6` / `muse-corpus-5` / `muse-occurrence-7` / canonicalization 7 / lowering 10.

Current toolchain-independent gates:

- 24 workspace/default-member and local lockfile consistency checks;
- generated/pinned foundation and UFO package verification;
- globally unique namespaceless learned ontology registry (492 concepts, 377 relations at this freeze);
- semantic-v6 source-shape verification, including the separate minimal learned target, exact normalization contracts, tool/update/result windows, singular ontology sorts, one relation channel, exact numeric grounding, ontology-backed scoped operators, source coverage, and compatibility-firewall checks;
- historical fixed 36-window prose-v4 regression: deterministic rebuild, structural validation, and source coverage;
- semantic-v6 adversarial audit: 12 pinned real-transcript cases spanning both language and structured-tool failure classes;
- Python syntax and shell syntax checks;
- combined cognitive/semantic static verification and root SHA-256 manifest after final regeneration.

The adversarial transcript work repaired tool exclusion, oversized structured records, lost split context/object identity, source alias duplication, root scalar grounding, provider visibility boundaries, lifecycle-update duplication, question-option representation, message-vs-delivery truth leakage, result failure/payload-truth conflation, generic `status` conflation, proposition range typing, and exact-type shell checks that rejected narrower ontology subtypes.

The complete Rust toolchain gate (`cargo check/test/clippy/rustdoc`, formatting, and MSRV/release checks) is still mandatory before training freeze. It is not claimed here because this execution environment does not provide `cargo`, `rustc`, or `rustfmt`.
