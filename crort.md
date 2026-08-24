For the second question, I think the present six are short of a good universal default set.

The current set is:

read
find
grep
write
move
poll

I would add at least these three:

Tool	Why it is missing
edit	Make a bounded change to an existing resource without replacing the whole thing.
run	Start an executable action or process and return resource URIs for its output/state.

edit is the most obvious one. The resource protocol already contains an Edit operation, but it is explicitly reserved and providers are told not to advertise it yet. So the architecture already expects this hole to be filled.

run is the larger missing capability. Right now the harness can manipulate resources, and poll is designed for streams such as process output, but there is no universal verb that creates such a process. That leaves poll without its most obvious producer.

I would make run fairly small. Something like:

run
  command
  args
  cwd?
  env?

and return something along these lines:

process://abc
process://abc/stdout
process://abc/stderr

Then the existing verbs compose cleanly:

run     -> start it
read    -> read current output
poll    -> wait for match/end

I would not add separate list, delete, or create verbs merely for completeness:

find with depth 1 already acts as list.
move with to = null already acts as delete.
write can create a resource where the provider allows it.

Whether those overloaded forms are good model-facing API design is a separate issue. In particular, move(..., to=null) meaning delete is compact, but not very explicit.

So my minimal universal set would be:

read
find
grep
write
edit
move
run
poll

Nine verbs.

There is one additional verb I would consider after those: call or invoke. That would represent “invoke a resource as an operation” for things that are neither files nor processes. But I would not add it until there is a concrete resource type that needs it. Otherwise it becomes an untyped escape hatch.

process:// seems suspect writ large, i would prefer more granular namespaces like bash:// or python:// but im unsure how to resolve this ergonomically with the run semantics you described

I agree that process:// is too generic if the scheme is supposed to tell you what kind of thing you are addressing.

I would separate two concerns:

the scheme identifies the execution provider
run is the generic verb that asks that provider to start work

So:

bash://
python://
node://
sql://

and run takes a target URI:

{
  "target": "bash://",
  "command": "cargo test",
  "cwd": "file:///repo"
}

The result stays in that provider's namespace:

bash://runs/7f3a
bash://runs/7f3a/stdout
bash://runs/7f3a/stderr

Then the existing universal verbs still work:

read  bash://runs/7f3a/stdout
poll  bash://runs/7f3a/stdout
read  bash://runs/7f3a

For Python:

python://runs/a812
python://runs/a812/stdout
python://runs/a812/stderr

That is cleaner than process://, because a URI keeps telling you which provider owns it.

The hard part is the run input schema. Bash, Python, SQL, and a browser runtime do not naturally take the same arguments. I would not force them into one fake universal shape like:

command + args

That works for executables but becomes awkward elsewhere.

I see three reasonable designs.

Generic run with provider-specific payload
{
  "target": "python://",
  "input": {
    "code": "print(1 + 1)"
  }
}

and:

{
  "target": "bash://",
  "input": {
    "command": "cargo test",
    "cwd": "file:///repo"
  }
}

This is extensible, but input becomes weakly typed unless the harness can expose the provider's schema.

Make the URI more specific and keep run small

For example:

{
  "target": "bash://local",
  "input": "cargo test",
  "cwd": "file:///repo"
}

and:

{
  "target": "python://3.13",
  "input": "print(1 + 1)",
  "cwd": "file:///repo"
}

This is much nicer if most executors can accept one primary text input. Provider-specific options can remain optional.

My preferred design is therefore close to option 2:

run
  target: URI
  input: string
  cwd?: URI
  env?: map<string,string>
  timeout_ms?: integer

The target determines semantics:

bash://local
python://3.13
node://22

The return value is a run URI in the same namespace:

{
  "uri": "bash://runs/7f3a"
}

Then define a small common structure below every execution URI:

bash://runs/7f3a
bash://runs/7f3a/stdout
bash://runs/7f3a/stderr

read on the run root could return structured state:

{
  "state": "running",
  "exit_code": null
}

After completion:

{
  "state": "finished",
  "exit_code": 0
}

That also gives poll a clean meaning. You can poll stdout for a regex:

poll bash://runs/7f3a/stdout match="Listening on"

or poll the run itself with no regex to wait for completion:

poll bash://runs/7f3a

Here, “close” becomes much less vague. For a stream URI, close means that stream will receive no more bytes. For a run URI, completion means the execution has reached a terminal state.

I would probably tighten the resource model around that distinction rather than making one generic closed state cover both.

The resulting default vocabulary would stay small:

read
find
grep
write
edit
move
run
poll

while execution types remain extensible through URI schemes:

bash://
python://
node://
browser://
sql://

The one capability I would add to make this ergonomic is scheme introspection. Something like:

inspect bash://

returning:

{
  "operations": ["run"],
  "input": "text",
  "supports": ["cwd", "env", "timeout"]
}

Then a new execution provider can be added without changing the run tool schema or adding another model-facing tool.

Why would we add a separate inspect verb instead of a query parameter for the info you want? Inspecting is just reading after all. //NOTE FROM ADAM: It's saying to use the ?something=something syntax here, which ik we broke already and im unsure if we should correct the breakage or continue not to use it---pls give your two cents.

I probably would not add a separate inspect verb.

If the resource model already says “everything is addressable by URI,” then metadata/state is another readable representation of that resource.

For example:

read bash://runs/7f3a?view=meta //ftr, 'runs' here is a completely useless intrinsic subdir.

could return:

{
  "state": "running",
  "exit_code": null,
  "operations": ["read", "poll"],
  "children": ["stdout", "stderr"]
}

Likewise:

read bash://?view=meta

could describe the provider:

{
  "operations": ["run"],
  "input": "text",
  "supports": ["cwd", "env", "timeout"]
}

That keeps the universal verbs smaller:

read
find
grep
write
edit
move
run
poll

My only reservation about query parameters is URI identity. You need a firm rule that something like:

bash://runs/7f3a
bash://runs/7f3a?view=meta

is the same underlying resource with a different representation, not two resources.

If that rule is explicit, I prefer your approach.

I would also avoid making every provider invent its own query syntax. Reserve a small harness-owned query namespace, perhaps:

?view=meta
?view=content

Then providers can still have their own parameters where needed, while generic resource introspection remains predictable.

So yes: inspect was redundant. read should cover it.
