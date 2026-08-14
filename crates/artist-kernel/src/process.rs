//! Direct executable-file process resources.
//!
//! This module deliberately accepts an executable path and argv separately.
//! It never invokes a shell, parses shell syntax, allocates a PTY, or exposes
//! a semantic host API beyond the process itself.

use crate::KernelError;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSnapshot {
    pub uri: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub aborted: bool,
}

struct ProcessState {
    child: Child,
    output: Arc<Mutex<Vec<u8>>>,
    aborted: bool,
}

fn spawn_reader<R: Read + Send + 'static>(mut stream: R, output: Arc<Mutex<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(size) = stream.read(&mut buffer) {
            if size == 0 {
                break;
            }
            if let Ok(mut current) = output.lock() {
                current.extend_from_slice(&buffer[..size]);
            }
        }
    });
}

#[derive(Clone, Default)]
pub struct ProcessManager {
    processes: Arc<Mutex<BTreeMap<String, Arc<Mutex<ProcessState>>>>>,
    next_id: Arc<Mutex<u64>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run(
        &self,
        executable: impl AsRef<Path>,
        args: &[String],
        cwd: Option<&Path>,
        environment: &[(String, String)],
    ) -> Result<String, KernelError> {
        let executable = executable.as_ref();
        if !executable.is_file() {
            return Err(KernelError::NotFound {
                uri: executable.display().to_string(),
            });
        }
        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command.envs(environment.iter().map(|(key, value)| (key, value)));
        let mut child = command.spawn().map_err(|error| KernelError::Handler {
            message: format!("could not execute {}: {error}", executable.display()),
        })?;
        let output = Arc::new(Mutex::new(Vec::new()));
        if let Some(stdout) = child.stdout.take() {
            spawn_reader(stdout, output.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(stderr, output.clone());
        }
        let mut next_id = self.next_id.lock().map_err(|_| KernelError::Handler {
            message: "process id lock poisoned".into(),
        })?;
        *next_id += 1;
        let uri = format!("process://{}", *next_id);
        drop(next_id);
        self.processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".into(),
            })?
            .insert(
                uri.clone(),
                Arc::new(Mutex::new(ProcessState {
                    child,
                    output,
                    aborted: false,
                })),
            );
        Ok(uri)
    }

    pub fn send(&self, uri: &str, input: &[u8]) -> Result<(), KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        process
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| KernelError::WrongKind {
                message: "process stdin is unavailable".into(),
            })?
            .write_all(input)
            .map_err(|error| KernelError::Handler {
                message: format!("could not write process stdin: {error}"),
            })
    }

    pub fn poll(&self, uri: &str) -> Result<ProcessSnapshot, KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        let exit_code = process
            .child
            .try_wait()
            .map_err(|error| KernelError::Handler {
                message: format!("could not poll process: {error}"),
            })?
            .and_then(|status| status.code());
        let output =
            String::from_utf8_lossy(&process.output.lock().map_err(|_| KernelError::Handler {
                message: "process output lock poisoned".into(),
            })?)
            .into_owned();
        Ok(ProcessSnapshot {
            uri: uri.to_owned(),
            output,
            exit_code,
            aborted: process.aborted,
        })
    }

    pub fn abort(&self, uri: &str) -> Result<(), KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        process.child.kill().map_err(|error| KernelError::Handler {
            message: format!("could not abort process: {error}"),
        })?;
        process.aborted = true;
        Ok(())
    }

    pub fn delete(&self, uri: &str) -> Result<(), KernelError> {
        let process = self
            .processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".into(),
            })?
            .remove(uri)
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_owned(),
            })?;
        if let Ok(mut process) = process.lock() {
            let _ = process.child.kill();
        }
        Ok(())
    }

    fn lookup(&self, uri: &str) -> Result<Arc<Mutex<ProcessState>>, KernelError> {
        self.processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".into(),
            })?
            .get(uri)
            .cloned()
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn executes_direct_file_without_shell_and_supports_process_lifecycle() {
        let manager = ProcessManager::new();
        let uri = manager
            .run("/bin/cat", &[], None, &[])
            .expect("cat executable should be available");
        manager.send(&uri, b"direct process\n").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            manager
                .poll(&uri)
                .unwrap()
                .output
                .contains("direct process")
        );
        manager.abort(&uri).unwrap();
        assert!(manager.poll(&uri).unwrap().aborted);
        manager.delete(&uri).unwrap();
        assert!(matches!(
            manager.poll(&uri),
            Err(KernelError::NotFound { .. })
        ));
    }
}
