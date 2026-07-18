//! The WASM plugin tier: wasmtime component host for programmable rules.
//!
//! Every plugin sits behind a mandatory native prefilter (regexes in its
//! manifest, compiled into the ordinary [`crate::matcher::RuleSet`]); the
//! guest is only consulted to *judge* a prefilter hit, never on the raw
//! delta path. Sandbox: WASI is linked with an EMPTY context — no
//! preopened directories, no environment, no args, no inherited stdio, no
//! network — so std-using guests instantiate but can reach nothing. The
//! only real host capabilities are `log` and a bounded session KV, under a
//! ~50ms epoch deadline and a 64 MiB memory cap per call. A trap,
//! deadline, or bad verdict poisons the plugin for the session; a broken
//! rule must never break the agent.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context as _, Result};

use crate::types::{Firing as RuleFiring, MatchTarget, Persistence, RuleId};

wasmtime::component::bindgen!({
    path: "wit/rule-plugin.wit",
    world: "rule-plugin",
});

const EPOCH_TICK_MILLIS: u64 = 10;
/// ~50ms of guest time per call.
const CALL_DEADLINE_TICKS: u64 = 5;
const MEMORY_CAP: usize = 64 * 1024 * 1024;
const KV_CAP_BYTES: usize = 64 * 1024;
const REMINDER_CAP: usize = 8 * 1024;

/// Host-side state for one plugin instance (the bindgen `host` interface).
struct HostState {
    plugin: String,
    kv: HashMap<String, String>,
    kv_bytes: usize,
    log: Vec<String>,
    limits: wasmtime::StoreLimits,
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime::component::ResourceTable,
}

impl wasmtime_wasi::WasiView for HostState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl host::Host for HostState {
    fn log(&mut self, message: String) {
        if self.log.len() < 64 {
            self.log.push(format!("[{}] {message}", self.plugin));
        }
    }
    fn kv_get(&mut self, key: String) -> Option<String> {
        self.kv.get(&key).cloned()
    }
    fn kv_set(&mut self, key: String, value: String) {
        let addition = key.len() + value.len();
        if self.kv_bytes + addition > KV_CAP_BYTES {
            // Evict arbitrary entries until it fits — bounded state, not a
            // database.
            while self.kv_bytes + addition > KV_CAP_BYTES {
                let Some(evict) = self.kv.keys().next().cloned() else {
                    break;
                };
                if let Some(old) = self.kv.remove(&evict) {
                    self.kv_bytes = self.kv_bytes.saturating_sub(evict.len() + old.len());
                }
            }
        }
        self.kv_bytes += addition;
        self.kv.insert(key, value);
    }
}

fn engine() -> &'static wasmtime::Engine {
    static ENGINE: OnceLock<wasmtime::Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).expect("baseline wasmtime config");
        // One background ticker drives every store's deadline.
        let ticker = engine.weak();
        std::thread::Builder::new()
            .name("artist-wasm-epoch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(EPOCH_TICK_MILLIS));
                    let Some(engine) = ticker.upgrade() else {
                        return;
                    };
                    engine.increment_epoch();
                }
            })
            .expect("spawn epoch ticker");
        engine
    })
}

/// One loaded plugin: an instantiated component and its session state.
pub struct WasmRule {
    pub id: RuleId,
    poisoned: AtomicBool,
    instance: Mutex<Instance>,
}

struct Instance {
    store: wasmtime::Store<HostState>,
    plugin: RulePlugin,
}

impl WasmRule {
    /// Load and instantiate a plugin component; verifies the guest's
    /// `meta()` id matches the manifest's rule name.
    pub fn load(id: RuleId, wasm_path: &Path) -> Result<Self> {
        let engine = engine();
        let component = wasmtime::component::Component::from_file(engine, wasm_path)
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .with_context(|| format!("load wasm component {}", wasm_path.display()))?;
        let mut linker = wasmtime::component::Linker::<HostState>::new(engine);
        RulePlugin::add_to_linker::<_, wasmtime::component::HasSelf<HostState>>(
            &mut linker,
            |state| state,
        )
        .map_err(|error| anyhow::anyhow!("{error:#}"))?;
        // Locked-down WASI so std-built guests can instantiate: an empty
        // context exposes no filesystem, environment, or sockets.
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|error| anyhow::anyhow!("{error:#}"))?;
        let mut store = wasmtime::Store::new(
            engine,
            HostState {
                plugin: id.0.clone(),
                kv: HashMap::new(),
                kv_bytes: 0,
                log: Vec::new(),
                limits: wasmtime::StoreLimitsBuilder::new()
                    .memory_size(MEMORY_CAP)
                    .instances(4)
                    .build(),
                wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
                table: wasmtime::component::ResourceTable::new(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(CALL_DEADLINE_TICKS);
        let plugin = RulePlugin::instantiate(&mut store, &component, &linker)
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .with_context(|| format!("instantiate {}", wasm_path.display()))?;
        let declared = plugin
            .call_meta(&mut store)
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .context("plugin meta() trapped")?;
        let expected = id.0.strip_prefix("wasm:").unwrap_or(&id.0);
        anyhow::ensure!(
            declared == expected,
            "plugin declares id `{declared}` but manifest names it `{expected}`"
        );
        store.set_epoch_deadline(CALL_DEADLINE_TICKS);
        Ok(Self {
            id,
            poisoned: AtomicBool::new(false),
            instance: Mutex::new(Instance { store, plugin }),
        })
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    /// Ask the guest to judge a prefilter hit. `None` = pass (including
    /// every failure mode — a broken plugin silently stops firing and is
    /// poisoned for the session).
    pub fn judge(&self, firing: &RuleFiring, turn: u32) -> Option<RuleFiring> {
        if self.is_poisoned() {
            return None;
        }
        let mut instance = self
            .instance
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Instance { store, plugin } = &mut *instance;
        store.set_epoch_deadline(CALL_DEADLINE_TICKS);
        let event = Event {
            target: firing.target.as_str().to_owned(),
            text: firing.matched.clone(),
            tool: None,
            turn,
        };
        match plugin.call_on_event(&mut *store, &event) {
            Ok(Verdict::Pass) => None,
            Ok(Verdict::Fire(fire)) => {
                let mut reminder = fire.reminder;
                if reminder.len() > REMINDER_CAP {
                    reminder.truncate(
                        (0..=REMINDER_CAP)
                            .rev()
                            .find(|index| reminder.is_char_boundary(*index))
                            .unwrap_or(0),
                    );
                }
                if reminder.trim().is_empty() {
                    self.poisoned.store(true, Ordering::Relaxed);
                    return None;
                }
                Some(RuleFiring {
                    rule: self.id.clone(),
                    target: firing.target,
                    matched: firing.matched.clone(),
                    reminder,
                    persistence: match fire.persistence.as_str() {
                        "message" => Persistence::Message,
                        _ => Persistence::Session,
                    },
                    fire: firing.fire,
                })
            }
            Err(_trap) => {
                // Deadline, memory cap, or guest panic: poison for the
                // session.
                self.poisoned.store(true, Ordering::Relaxed);
                None
            }
        }
    }

    /// Diagnostic lines the guest logged (drained).
    pub fn drain_log(&self) -> Vec<String> {
        let mut instance = self
            .instance
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut instance.store.data_mut().log)
    }
}

/// The manifest sitting beside `<name>.wasm` in a rules directory.
#[derive(serde::Deserialize)]
pub struct WasmManifest {
    pub description: String,
    /// Mandatory native prefilter regexes — the plugin is only consulted
    /// after one of these matches.
    pub prefilter: Vec<String>,
    #[serde(default)]
    pub targets: Option<Vec<MatchTarget>>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub fire: Option<crate::types::FirePolicy>,
    #[serde(default)]
    pub scope: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[allow(unused)]
fn assert_traits() {
    fn requires_send_sync<T: Send + Sync>() {}
    requires_send_sync::<WasmRule>();
}
