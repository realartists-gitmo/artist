# Semantic-v6 adversarial audit

This is the active pre-label adversarial gate. `cases.json` pins real records from `agentic-sessions-big-37(2).zip` by source path, line, byte length, SHA-256, and an exact source needle.

The cases are deliberately chosen to break weak semantics rather than demonstrate easy labels. They cover lexical/modal scope, numeric/focus distinctions, measurement/range structure, structured question/options, tool lifecycle identity, requested-vs-observed effects, embedded agent prompts, schema-dependent status, partial failure observations, alias normalization, provider lifecycle status, and encoded structured results.

Run:

```sh
python3 audit/semantic-v6/scripts/verify_adversarial_draw.py
```

If the transcript archive is available, pass `--source-archive PATH`; the verifier checks every pinned raw line exactly. The workspace verifier uses the archive automatically when it is present at the session working path.
