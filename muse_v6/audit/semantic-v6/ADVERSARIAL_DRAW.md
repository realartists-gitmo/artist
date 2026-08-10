# Semantic-v6 adversarial draw

Status: normative adversarial pre-label gate.

## 1. `language_modal_optimization_plural_scale`

Source: `grok/%2Fhome%2Fadam%2FProjects%2Fdebateasr/019f829e-1715-7be1-a4d6-2c66fe408203/updates.jsonl:1`

Pinned SHA-256: `c9489639016c67623b9ed815b443a7dfe5bcb4830c19e227a003be86dfb715ef`

Needle: `As far as we know`

Required invariants:
- epistemic qualification is scoped force, not logical necessity
- 1000s is plural-scale cardinality, not Approximately(1000)
- as many as possible uses MaximizationOperator + optimizationTarget

## 2. `language_generic_capability_approx_focus`

Source: `grok/%2Fhome%2Fadam%2FProjects%2Fdebateasr/019f829e-1715-7be1-a4d6-2c66fe408203/updates.jsonl:54`

Pinned SHA-256: `ab8449ac09d9848fd7dfa2f788e350198ad67cc5b30fa1754bec9da290d28609`

Needle: `5 or so personal youtube accounts`

Required invariants:
- as a rule is GenericOperator
- can is CapabilityOperator when source licenses ability
- 5 or so is approximate cardinality with separate cue
- only 500 is ExclusiveFocusOperator over exact cardinality, not just Exactly(500)

## 3. `language_measurement_interval`

Source: `grok/%2Fhome%2Fadam%2FProjects%2Fdebateasr/019f829e-1715-7be1-a4d6-2c66fe408203/updates.jsonl:26`

Pinned SHA-256: `c557e33e877a3f52e3c32b93a6653731e5ac829894466cdf5f338cdedefeec96`

Needle: `~1.5 s/query`

Required invariants:
- approximate measurement preserves unit
- 20-60 s is NumericInterval measurement, not two unrelated numbers

## 4. `tool_question_options`

Source: `grok/%2Fhome%2Fadam%2FProjects%2Fbountyhunting/019f8e56-b3d4-7743-a2a0-c0f07599974f/updates.jsonl:94`

Pinned SHA-256: `17de95c5b19a4388e0a9f18df060fca377acb7ba9ecff910e82ccf10ac2ab843`

Needle: `What autonomy level do you want`

Required invariants:
- question/options are learned structured content
- choice options are alternatives, not assertions

## 5. `tool_lifecycle_update`

Source: `grok/%2Fhome%2Fadam%2FProjects%2Fbountyhunting/019f8e56-b3d4-7743-a2a0-c0f07599974f/updates.jsonl:3218`

Pinned SHA-256: `37b24827b0370cf7e5c46129c0e64534008a3ecccee1626392380a00cacd97d9`

Needle: `tool_call_update`

Required invariants:
- update is not a second invocation
- compiler-owned invocation identity links update downstream

## 6. `tool_requested_edit`

Source: `claude/-home-adam-Projects-debateDB/494f31c9-1c53-4330-861e-1c52e96f2ecc.jsonl:691`

Pinned SHA-256: `b964d429ccad6d901ee0e68c30745b23b58adeffff57d0fc562435f95a4878d3`

Needle: `"name":"Edit"`

Required invariants:
- requested write is intensional requested effect
- call does not assert file mutation occurred

## 7. `tool_delegation_prompt`

Source: `claude/-home-adam-Projects-debateDB/494f31c9-1c53-4330-861e-1c52e96f2ecc.jsonl:6378`

Pinned SHA-256: `ed12bb5e53b203f6b94a9d4886b6d59f5e1bdc95ec84b84579bf6f49ed70bdfb`

Needle: `"name":"Agent"`

Required invariants:
- embedded prompt semantics stay scoped inside communicated/requested content
- prompt assertions do not leak to top-level truth

## 8. `tool_task_status`

Source: `claude/-home-adam-Projects-debateDB/494f31c9-1c53-4330-861e-1c52e96f2ecc.jsonl:6486`

Pinned SHA-256: `143a285aca23fcb4d6ebdc38a941af9395b8114c5bd9b1d1d87d63a81b96929f`

Needle: `"status":"in_progress"`

Required invariants:
- tool-specific requested state is not a generic observed status fact

## 9. `tool_partial_failure`

Source: `claude/-home-adam-Projects-debateDB/494f31c9-1c53-4330-861e-1c52e96f2ecc.jsonl:246`

Pinned SHA-256: `9b0acf061648a74216cd5637ecca1a549c400efd6dd74552524c9ec920fb6dc0`

Needle: `Exit code 143`

Required invariants:
- execution outcome is separate from payload propositions
- partial observations remain representable despite failure

## 10. `tool_message_aliases`

Source: `claude/-home-adam-Projects-debateDB/494f31c9-1c53-4330-861e-1c52e96f2ecc.jsonl:7298`

Pinned SHA-256: `ff29ac5622e198a586fa12c7d77e2f1994925fe3e09451b510a6e654995af991`

Needle: `"name":"SendMessage"`

Required invariants:
- schema-declared aliases canonicalize only after equality proof
- message artifact/requested delivery does not assert successful delivery

## 11. `tool_provider_status`

Source: `codex/2026/06/29/rollout-2026-06-29T04-47-15-019f12c6-a3d0-7310-ac6c-b48e6ada3d20.jsonl:202`

Pinned SHA-256: `e8c31c15f8599177f6c89fae05772ea145ac84a13b377288593421867bf080cf`

Needle: `"status":"completed"`

Required invariants:
- provider lifecycle status does not imply patch effect success
- patch body is learned tool content

## 12. `tool_large_encoded_result`

Source: `codex/2026/08/01/rollout-2026-08-01T23-41-40-019fc0c7-16ac-7a80-8dbe-803df80efc07.jsonl:24`

Pinned SHA-256: `72f514161d5a8fc196a16cb0b5cf5911538cacd2e2d23eb8b48fee44bb8bf981`

Needle: `function_call_output`

Required invariants:
- structured payload is normalized by pinned visibility contract
- JSON-looking strings are not recursively parsed without a reversible contract

