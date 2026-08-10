# Semantic-v5 adversarial draw and formalism repair

Status: active pre-label adversarial gate for `muse-semantic-label-5`.

The purpose of this audit is to break the labeling contract before corpus annotation. A source window that forces fact leakage, duplicate event identity, ontology backoff, provider-envelope guessing, or loss of exact source grounding is a formalism/windowing defect, not an annotator-quality problem.

## First draw: defects found and repaired

The first draw (`draw.json`) exposed these distinct failures:

1. **Tool exclusion was invalid.** Tool calls/results can contain natural-language messages, prompts, questions, reports, plans, and diagnostics. `TrainingWindow` now includes structured tool calls, lifecycle updates, and results.
2. **Actor and recorder were conflated.** Structured records now distinguish semantic actor/invoker from recorder/presenter; `Tool` is a first-class source role.
3. **One record could exceed model context by orders of magnitude.** Structured payloads are flattened by RFC-6901 field path and partitioned; large strings are UTF-8/discourse-safe split without dropping bytes.
4. **Partitioning destroyed object context.** Every part repeats tool/invocation context, carries bounded scalar sibling/ancestor context, and exposes enclosing container paths as stable whole-object anchors.
5. **Opaque binary was tokenized as text.** Only source-schema-proven binary/blob fields may be externalized to encoding/media-type/digest/length metadata.
6. **Aliases duplicated semantics.** Source-schema-declared equal aliases are losslessly canonicalized to one model-visible value while every source alias remains a legal grounding path.
7. **RFC-6901 root scalar payloads were ungroundable.** Empty root pointer `""` is now a legal structured source path.
8. **Raw provider logs were mistaken for model-visible context.** Corpus adapters must reconstruct the exact model-visible call/result surface and keep transport telemetry/internal caches as provenance.
9. **Question options had no semantic home.** `QuestionPrompt` and `ChoiceOption` plus prompt/option relations represent offered alternatives without asserting or selecting them.
10. **Lifecycle updates became duplicate invocations.** `ToolCallUpdate` / `ToolInvocationUpdate` are distinct from `ToolCall` / `ToolInvocation`; exact invocation-ID joins occur downstream.
11. **Proposition-valued ontology relations were not actually range-checked.** `Term::Proposition` now deterministically types as `ufo:Proposition` at the formal boundary.
12. **Message existence implied delivery.** `Message` is an information artifact; `MessageDelivery` is a separate communicative act. A send call can represent requested delivery without asserting delivery success.
13. **Structured shell checks forced generic root types.** Root validation now uses ontology ancestry, so a justified `PatchToolInvocation`, `ShellToolInvocation`, or `AgentCoordinationToolInvocation` satisfies the `ToolInvocation` shell without backing off to the ancestor.

## Second draw: adversarial cases

`draw2.json` deliberately targets different failure modes after those repairs:

- Claude `Edit`: requested code/file transformation versus completed filesystem effect.
- Claude `Agent`: delegation prompt with nested directives, questions, and quoted identifiers.
- Claude `TaskUpdate`: a requested task state, not proof that harness task state already changed.
- Claude `WebFetch`: a network/tool request whose extraction prompt is not yet a returned fact.
- Claude failed/timed-out Bash result: global failure status plus valid partial observations emitted before failure.
- Claude `SendUserFile`: requested communication/delivery versus successful delivery.
- Codex `apply_patch`: provider response-item `status=completed` versus patch-effect success.
- Codex function-call output containing JSON encoded inside a text item: reversible transport normalization versus invented nested source paths.
- Grok cumulative tool-output stream snapshots: UI/transport visibility versus genuine model-visible lifecycle updates.

The second draw forced two further repairs:

1. **Result failure cannot negate payload observations.** `toolResultOutcome : ToolResult -> ExecutionOutcome` now represents source-reported operation status independently of returned reports/data. A timed-out command can be a `CommandFailure` while still reporting observations produced before timeout.
2. **`status` is not a universal semantic key.** Status interpretation is source-schema/tool-specific. Requested task state, lifecycle-update state, provider item completion, and tool-result outcome are distinct meanings and may not be conflated by field-name heuristics.

The Grok stream case also tightened normalization: cumulative UI/transport snapshots are not learned merely because they exist in a raw event log. If an audited adapter proves that the model actually received them, they remain separate `ToolCallUpdate` records; otherwise the adapter reconstructs the actual model-visible result and retains the stream only as provenance.

## Corrected target sketches

These sketches omit exact source-span IDs and canonical local node IDs. Production labels retain them.

### A. Claude `Edit` tool call

The call establishes the invocation. The requested file write does not become an observed write.

```text
invocation : CodeTransformationToolInvocation
edit_tool  : Tool
actor      : Agent
path       : Path
requested_write : FileWrite

Ontology(toolInvocationPrincipal, invocation, actor)
Ontology(invokesTool, invocation, edit_tool)
Ontology(toolInvocationEffect, invocation, requested_write)
Ontology(writePath, requested_write, path)

p_write   = Occurrence(requested_write)
p_request = SpeechAct(
    speaker    = actor,
    act        = Request,
    addressees = {edit_tool},
    content    = p_write
)

Record(Occurrence(invocation))
Record(p_request)
```

`old_string` and `new_string` remain exact structured string/data arguments unless the pinned source-code/patch ontology licenses a narrower artifact representation. No `FileWrite` is asserted as completed from the call alone.

### B. Claude `TaskUpdate {taskId, status=in_progress}`

```text
invocation    : TaskManagementToolInvocation
task_tool     : Tool
actor         : Agent
task          : Task
desired_state : TaskState

p_state_1 = Relation(taskStateOf, desired_state, task)
p_state_2 = Relation(taskStateValue, desired_state, String("in_progress"))
p_state   = Conjunction({p_state_1, p_state_2})
p_request = SpeechAct(actor, Request, {task_tool}, p_state)

Record(Occurrence(invocation))
Record(p_request)
```

The source string `taskId` grounds the `Task` referent. It does not require the SLM to copy the opaque provider identity into model-generated external-ID metadata. `in_progress` is desired/requested state here, not an independently observed current state.

### C. Claude timed-out Bash result with useful output

Assuming an exact tool-use-ID join establishes the Bash call:

```text
result  : ToolResult
outcome : CommandFailure
bash    : Tool

Relation(toolResultOutcome, result, outcome)

# separately, payload text reports observations emitted before timeout
p_report = SpeechAct(
    speaker = bash,
    act     = Report,
    content = <semantics of the source-reported observations>
)

Record(Relation(toolResultOutcome, result, outcome))
Record(p_report)
```

`CommandFailure` does not make the report content false. Each payload claim is labeled according to its own source/report force.

### D. Codex `apply_patch` call with response-item `status=completed`

```text
invocation : PatchToolInvocation
patch_tool : Tool
patch      : PatchArtifact
apply      : PatchApplication

Relation(toolInvocationEffect, invocation, apply)
Relation(patchUsesArtifact, apply, patch)

p_apply   = Occurrence(apply)
p_request = SpeechAct(actor, Request, {patch_tool}, p_apply)

Record(Occurrence(invocation))
Record(p_request)
```

The provider item's `status=completed` does **not** license `PatchApplication` success. Actual patch outcome requires the result/source that reports it.

### E. Claude `Agent` delegation prompt

```text
invocation : AgentCoordinationToolInvocation
agent_tool : Tool
actor      : Agent
task       : Task
delegate   : DelegationAction
prompt     : Message

Relation(toolInvocationEffect, invocation, delegate)
Relation(delegatesTask, delegate, task)
Relation(messageSender, prompt, actor)

# The prompt's "report ...", questions, and "do not modify" are scoped
# communicative/task content, not independent world facts.
p_task_request = SpeechAct(actor, Request, {agent_tool}, <task/prompt proposition>)

Record(Occurrence(invocation))
Record(p_task_request)
```

A successful later agent result may establish that an execution/delegation happened; the call itself establishes the request/invocation and task/message artifacts.

### F. Grok cumulative output updates

Two raw UI stream events with the same invocation ID and identical final payload but statuses `in_progress` then `completed` do not justify two tool invocations. The source adapter first answers the visibility question:

```text
if transport/UI-only:
    provenance only; reconstruct actual model-visible result

if genuinely model-visible:
    update_1 : ToolInvocationUpdate(status="in_progress")
    update_2 : ToolInvocationUpdate(status="completed")
    # both can reconcile to the same ToolInvocation via exact invocation identity
```

No path creates a second `ToolInvocation` merely because an update record arrived.

## Bounce policy after the second draw

A labeller must bounce rather than invent semantics when:

- provider visibility cannot be established for a raw field/event (`BOUNCE: SOURCE/NORMALIZATION`);
- partitioning removes the semantic object/context needed to produce one grounded target (`BOUNCE: WINDOWING`);
- a needed concept/relation cannot be represented even at intended foundational precision (`BOUNCE: ONTOLOGY`);
- requested versus observed effect, quotation/report scope, lifecycle identity, or another truth-conditional distinction cannot be encoded (`BOUNCE: FORMALISM`).

Ordinary lexical ambiguity remains `ACCEPT WITH AMBIGUITY`, not a bounce.
