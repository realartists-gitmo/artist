# Validation procedure

This source package does not embed a historical claim that a particular machine ran the suite. Run the following on the exact extracted archive:

```bash
./scripts/verify.sh
python3 scripts/static_verify.py
```

Record the Rust toolchain, operating system, command output, archive digest, and date in the consuming environment. The static verifier checks workspace consistency, balanced Rust source, forbidden placeholders/unsafe blocks, and required capability declarations. It does not replace compilation, tests, or Clippy.
