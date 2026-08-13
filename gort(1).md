 1. Is file:// the canonical internal form for OS files, with bare paths accepted only at the outer adapter? // Yes, but bare paths always = file:// namespace, they should be accepted everywhere.
  2. Do anchors survive process restarts, or are they only valid for the lifetime of the resource instance?  // The calculation is stateless w/r/t the file. If the file doesnt change, it's mathematically impossible for the anchors to change, man.
  3. Does moving a line preserve its anchor, or count as delete-plus-create? // Sometimes it might, but sometimes it'll alter the typekind content we feed into teca. This is obvious. Are you sure you understand the anchors?
  4. How should ambiguous anchor reconciliation after external edits behave: deterministic best effort or invalidate rather than guess? // Invalidate
  5. What are the exact first shared WIT types and interfaces? The document names them, but does not yet provide the authoritative WIT definitions. // What should they be based on the spec?
  6. Are large AnchoredText results materialized, streamed, or eventually represented as resource references? // What does this mean man
  7. Is the initial verb set exactly the ten listed verbs? // Yes
  8. What resources may run execute: OS files, WASM components, repo resources, or all resource types that advertise executability? // All
  9. What working directory/environment/authority does run inherit? // IDK man, what makes sense?
  10. Does Changed in poll include replacements and deletions, or only appended lines? // What?
  11. Does Regex inspect accumulated observed text or only newly arriving text? // Newly-arriving. Grep is for old text
  12. What precisely does poll return when its condition succeeds: all text since the cursor, only newly arrived text, or a final snapshot? // A window around its condition. Is this seriously not in the spec?
  13. Are process termination semantics the only initial implementation of Terminated? // What?
  14. Is there one active implementation per verb contract, or can implementations be selected by URI/resource/backend? // What...?
  15. Can a component implement multiple verbs? // What? No.
  16. Are component imports statically linked at activation or dynamically resolved through the registry? // Dynamically resolved IF i understand what you mean, but this question is retardedly confusing, pls lock in.
  17. Are imported tool dependencies pinned for the full invocation during reload? // What?
  18. How are contract versions identified and compatibility checked? // IDFK
  19. Does editing tools:// source trigger automatic reload, or is reload initially explicit? // IDFK i havent decideed
  20. How is a contract/WIT change installed separately from an implementation-only replacement? // what?
  21. Are old versions retained only while leased, or do we need rollback history? // no rollback history
  22. Which first proving slice do we want: read alone, or find → read composition? // uh... 'proving slice' is bad epistemology. Let's just build the system that will be good.
