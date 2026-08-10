# Build status

## Source status

The runtime design is implemented around the final agreed invariants:

- every harness write is exactly ontologically typed and structurally interpreted;
- ontology meaning is fixed before theories are selected;
- one dependent-type kernel checks deductive certificates;
- theory extensions add domain content without reinterpreting ontology;
- universal proof/computation traces cannot bypass the DTT certificate chain;
- Mnestic storage remains external;
- Lean is not a permanent project component.

## Validation status

No executable validation result is asserted by this file. Run `./scripts/verify.sh` and `python3 scripts/static_verify.py` in the target environment. The user requested local execution testing, so archive consumers should treat local results as authoritative.
