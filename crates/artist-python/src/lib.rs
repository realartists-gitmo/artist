//! Native PyO3 transport used from the separate Artist Python worker process.
//!
//! The process boundary is intentional: this module never embeds Python in the
//! harness. It only exchanges one JSON RPC request/response over inherited pipes.

use std::io::{BufRead, Write};

use pyo3::{exceptions::PyRuntimeError, prelude::*};
use serde_json::{Value, json};
use uuid::Uuid;

#[pyfunction]
fn call(method: &str, params_json: &str) -> PyResult<String> {
    let params: Value = serde_json::from_str(params_json)
        .map_err(|error| PyRuntimeError::new_err(format!("invalid params JSON: {error}")))?;
    let id = Uuid::new_v4().to_string();
    let message = json!({
        "kind": "rpc",
        "id": id,
        "method": method,
        "params": params,
    });
    {
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        serde_json::to_writer(&mut output, &message)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        output
            .write_all(b"\n")
            .and_then(|_| output.flush())
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
    }

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
    let response: Value = serde_json::from_str(&line)
        .map_err(|error| PyRuntimeError::new_err(format!("invalid harness response: {error}")))?;
    if response.get("kind").and_then(Value::as_str) != Some("rpc_result")
        || response.get("id").and_then(Value::as_str) != Some(&id)
    {
        return Err(PyRuntimeError::new_err("mismatched harness RPC response"));
    }
    if let Some(error) = response.get("error").and_then(Value::as_str) {
        return Err(PyRuntimeError::new_err(error.to_owned()));
    }
    serde_json::to_string(response.get("value").unwrap_or(&Value::Null))
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

#[pymodule]
fn _artist_bridge(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(call, module)?)?;
    Ok(())
}
