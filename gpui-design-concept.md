Left sidebar:

Interactive tree of the stuff.

We'll define the stuff as 'sessions'. A session will be a fundamental UX primitive for us. Everything on the tree is a 'session.'

Items on tree include:
Agents
Subagents
Tasks
Active computer use sessions
Canvases

The top level of the tree is the main agent. Every thing else is a child of the agent that created it. The top-level siblings establish the history.

Simplify the LLM autism of items on the tree, don't need that large of elements, that much info, or that much clutter. If you want to know what gpui design patterns our team finds aesthetic, look for general principles in the flowstate project one level up. Here's need to know.
Artist name (like Picasso, or Bach---should be able to understand what these are by reading the code if this cofnuses you)
A small string from the last recognizable text in the session, so that would be either human prompts or model dialogue. This updates live as it streams, and truncates if it goes beyond the sidebar width.
How long ago since the session was active
What profile the specific item is

What does it mean for a session to have been active?
Agents: running
Subagents: running
Tasks: running
Computer use: interacted with
Canvases: opened.

The files list is unnecessary, gut it.

Swap to gpui component assets icon-based ui as much as possible. Textual ui interfaces is a very common symptom of LLM autism.

I have no clue what the right sidebar is meant to be for coherently. What is it for??

Is the 'surface' for interacting with an agentic coding harness in all the ways users typically expect to interact with one really ergonomic here, or is bullshit hidden in random places, disunified, and separated against the mental model (hint: it's the second one)>

Important:

You may have noticed that the window into artist stages is currently a separate window. You may also have noticed that the canvases element renders as its own webapp in a separate window as well. My vision is to bring both of these as nested gpui divs into the project itself. For the first thing (streaming the compositor the agent drives and allowing user input to come through it), I have no clue what crates or libs are available. For the second one, I'm like 99% sure we should use gpui-component's webview feature, and use the experimental wef backend since Linux is a primary target.

You're gonna mockup a map of what the actual design architecture you'll refactor towards is, and you'll need me to approve it explicitly.
