# Subagents

The former subagent implementation was removed during the VFS reset. Agents
will return as addressable `agent://` nodes created through the unified write
contract, with lifecycle and results exposed through the common filesystem
verbs.
