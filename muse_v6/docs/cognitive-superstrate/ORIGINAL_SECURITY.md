# Security policy

The kernel accepts untrusted serialized theories, terms, certificates, graphs, and checkpoints. Treat all of them as attacker-controlled input.

Required deployment controls:

- limit serialized input size at the protocol boundary;
- run checking with resumable step budgets and host cancellation;
- preserve exact theory/checkpoint pairing;
- do not load executable code from theory packages into the trusted process;
- disclose every axiom and oracle before using a result for authorization;
- validate canonical hashes after storage retrieval;
- treat resource exhaustion as suspension or rejection by policy, never falsity;
- keep proof discovery and empirical engines in less-trusted processes when practical.

No unsafe Rust is permitted in this workspace.
