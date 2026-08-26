"""Artist's persistent JSON-lines Python worker.

Protocol traffic uses the original stdin/stdout. User stdout/stderr is captured so it
cannot corrupt the protocol. This process is disposable; durable truth lives in Rust.
"""

import ast
import contextlib
import io
import json
import os
import sys
import traceback
import types
import uuid

_PROTOCOL_IN = sys.stdin
_PROTOCOL_OUT = sys.stdout


def _send(value):
    _PROTOCOL_OUT.write(json.dumps(value, default=repr, separators=(",", ":")) + "\n")
    _PROTOCOL_OUT.flush()


try:
    import _artist_bridge
except ImportError:
    _artist_bridge = None


def _rpc(method, params=None):
    # Installed environments use the small native PyO3 bridge. The Python path
    # is retained for source-tree development before `maturin develop`.
    if _artist_bridge is not None:
        return json.loads(_artist_bridge.call(method, json.dumps(params)))
    request_id = str(uuid.uuid4())
    _send({"kind": "rpc", "id": request_id, "method": method, "params": params})
    line = _PROTOCOL_IN.readline()
    if not line:
        raise RuntimeError("Artist harness disconnected during RPC")
    response = json.loads(line)
    if response.get("kind") != "rpc_result" or response.get("id") != request_id:
        raise RuntimeError("Artist harness returned an invalid RPC response")
    if "error" in response:
        raise RuntimeError(response["error"])
    return response.get("value")


artist = types.SimpleNamespace(
    call=_rpc,
    spawn=lambda node_id, input: _rpc("spawn", {"node_id": node_id, "input": input}),
    fork=lambda inputs: _rpc("fork", {"inputs": inputs}),
    fork_and_join=lambda inputs: _rpc("fork_and_join", {"inputs": inputs}),
    join=lambda run_ids: _rpc("join", {"run_ids": run_ids}),
    replace=lambda successors: _rpc("replace", {"successors": successors}),
)

try:
    import fff as fff
    _FFF_AVAILABLE = True
    _FFF_ERROR = None
except Exception as error:
    fff = None
    _FFF_AVAILABLE = False
    _FFF_ERROR = repr(error)

_globals = {
    "__name__": "__artist_repl__",
    "artist": artist,
    "fff": fff,
}

_send({
    "kind": "ready",
    "pid": os.getpid(),
    "fff_available": _FFF_AVAILABLE,
    "fff_error": _FFF_ERROR,
    "native_bridge": _artist_bridge is not None,
})

for line in _PROTOCOL_IN:
    try:
        request = json.loads(line)
    except Exception as error:
        _send({"kind": "result", "ok": False, "error": f"invalid request: {error}"})
        continue
    if request.get("kind") == "shutdown":
        break
    if request.get("kind") != "execute":
        _send({"kind": "result", "id": request.get("id"), "ok": False, "error": "unknown request"})
        continue

    stdout = io.StringIO()
    stderr = io.StringIO()
    result = None
    ok = True
    error_text = None
    try:
        tree = ast.parse(request.get("code", ""), mode="exec")
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            if tree.body and isinstance(tree.body[-1], ast.Expr):
                expression = ast.Expression(tree.body.pop().value)
                if tree.body:
                    exec(compile(tree, "<artist-repl>", "exec"), _globals, _globals)
                result = eval(compile(expression, "<artist-repl>", "eval"), _globals, _globals)
            else:
                exec(compile(tree, "<artist-repl>", "exec"), _globals, _globals)
    except BaseException:
        ok = False
        error_text = traceback.format_exc()

    serializable = result
    try:
        json.dumps(serializable)
    except Exception:
        serializable = {"repr": repr(result), "type": type(result).__name__}
    _send({
        "kind": "result",
        "id": request.get("id"),
        "ok": ok,
        "value": serializable,
        "stdout": stdout.getvalue(),
        "stderr": stderr.getvalue(),
        "error": error_text,
    })
