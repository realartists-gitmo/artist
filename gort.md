Trying to access divine beauty

Everything is a path

Idea from oh my pi / omp--but MUCH more aggressive---omp does it for convenience over some things---we will use it as a MANDATORY and UNIVERSAL addressing space for everything the harness touches that isn't a real file or path,

The virtual filesystem handles FS-shaped tools uniquely for each type of thing/request

repo://username/project/pr/1 when read shows the PR
Same analysis rules as omp here

Sessions are also paths

Adjustment to poll()---a study found that models can actually adjust the terms of terminating their poll() on streaming input to about a 30% gain. Obviously polls should finish when the session closes entirely as they do now, or when the timeout elapses as we have now, but we should also explore and figure out the ergonomics for finishing a poll on matching against the stream.

Anchor primitive holds through everything---to address an anchor, we append # to the end and put the anchor, range of anchors, or ranges of anchors after it. This is TECHNICALLY useless for immutable resources, BUT it prevents dual-authority confusion AND prevents us by construction from ever exposing line numbers to the model accidentally for a concept that's actually more mutable than we thought.

bash:// *swap for brush and uutils as we do this

canvas://
canvas://active/
canvas://idle/
canvas://templates/

debug://

eval:// (evxcr REPLs, we'll depend on evxcr proper, tool-calling built-in obviously--no JS or python because we are working on the soft intuition that rust prevents more model errors by construction, gives more verbose and useful error output, and that agents writing rust are more likely to think with patterns characteristic of smart people, vice versa for python and JS)

profile:// (or should it be /profiles/ under agent? maybe as pointers? or no? or should each profile contain pointers to every agent with its profile. damn this is hard)

skill://

agent://

rule:// (for TTSRs, need to fix these up a little imo)

mcp://

todo://
^ Honestly, the entire todo tool surface should probably be replaced with reading, writing, and editing to locations in the todo FS.

^ Main agents are paths as well. An agent can see itself via a path!

^ Obviously all of these require unique hierarchy schemas being defined once divinely, so we need to think about that and lock it in.

Hell, we might even throw 

tools://

in, and let the model read(), write() or edit() its own tools. Would need the WASM extension logic worked out pronto, but research on self-modifying harnesses shows they're really powerful

It's a relational DB, so you can traverse pointers to recursively look up important info. Do depend on mnestic (maintained fork of cozo) with a rocksDB backend now for this, or is that too much and we should implement the entire relational graph ourself?

For example

agent://Goethe/children/agents/* <- enumerates all immediate subagents of goethe, which are really just pointers to their locations in agent://, so agent://Goethe/children/agents/DOOM is just a pointer to agent://DOOM

conversely, you can unwrap agent://DOOM and find its parent, though i havent decided what the structure of finding parents should be---parents can be anything, not just other agents, since subagents can be spawned by bash, but each existing thing will pretty much have exactly one parent.

^ also need predecessor/successor for handoffs

Big ast-bro tool surface? Mostly paths. You can look into the symbols, callees, callers, dependencies, and so on. NEEDS to point to accurate anchors, or we're raped.

All addressing will occur via paths

Rather than authored skill finding machinery, if the agent wants to find a skill, it just finds in skill://

So we increase the target space, and vastly decrease the tool surface

Idea initially created by the omp devs, and it is utter genius, but requires a lot of engineering

To grab PRs from github, your 'filesystem' must actually be interacting with github APIs under the hood. Or for other forges, it must interact with those APIs. It actually has to figure out which API to use for a repo to avoid that causing the model strain or trouble.

Tool semantics:
- read() is for ~immediate, static output based on the condition at that moment. We need to redesign specify about how much text read extracts, which text it extracts, and how we allow the model to select to read new text and whatnot. Read for streaming output ofc just means the state of output at that very moment.
- edit() for changing mutable files via anchor-addressed edits
- write() for creating new files in mutable dirs or overwriting mutable files entirely
- poll() is a blocking variation on read, for streamed/live sources of output that the agent wants to await something from---we want polls to be able to do all three of these things: wait till idle, timeout at a certain cap, or regex match on a specific streamed output---in harmony
- send() is for interactive input---messages to subagents, commands into bash shells, input that'll be parsed by canvases, replaces evaling into the REPL, etc.
- abort() is for killing a session-shaped thing---we might want to rename it to delete() and also allow it to delete files and abort sessions since we're pursuing this unification.
- find() for looking for paths and files
- grep() for looking inside paths and files
(List will nicely be removed)

Open design questions to lock in BEFORE this refactor:
Should we keep the separate authored spawn tools for each session type? It feels like if a bash script has been written, or a specific subagent profile has been written for the project, a pathed generic spawn() tool might be more ergonomic than forcing the rewrite of the solution each time (and it does narrow the tool surface). Which points me towards one spawn() tool over items in the filetree. But then how do we handle one-off subagents that adopt a profile but need a prompt tweak? Immediate spawn then a separate tool send is too clunky and cancerous imo.
The computer use tool surface is a mess---we need to implement the new uni-CLI style rung 0, then fix the tool surface altogether.
Retrieval and ingress context filters. We are definitely gonna err toward snapcompact for compaction, but in the field of tools like clawcompact, omp's authored filter, and RTK, there isn't really one clearly divine solution for just simply reducing input tokens the model gets. The thing is---we can't use the dynamic solutions, because tools being called by other tools means input-output has to be programmatic and predictable based on the model-facing shape, so we need to figure out THE divine and permanent ingress-retrieval filter architecture NOW
NEED better structural contracts for the ast query and ast edit tools that cant be subsumed as paths. NEED
NEED to make sure simultaneous reads and edits are first-class desperately, because batching edits in the same refactor is better for avoiding interrim diagnostic slop
NEED to make every output more easily programmatic (including subagents) by trying to get universal structured output. for subagents yield() tool vestige was an attempt at this, but may be wrong primitive. idk what the right one is.

Other things we MUST do:
- LSP integration like OMP has, with MUXING MANDATORY--SCOPING TO AGENTS WITH BLAME ALSO NECESSARY
- debugger DAP client like OMP has, is a session
- FIX subagent profiles
- fix and FILL OUT ttsrs
- finish memory eventually
- add snapcompact
