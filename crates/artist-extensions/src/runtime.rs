use crate::{DiscoveredExtension, EventBus, ExtensionContext, HostControl};
use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    sync::Mutex,
};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, HasSelf, Linker, ResourceTable},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    path: "wit", world: "artist-extension",
    imports: { default: async }, exports: { default: async },
});

struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    children: HashMap<u64, Child>,
    next_child: u64,
    control: Arc<dyn HostControl>,
    context: Arc<RwLock<ExtensionContext>>,
    events: EventBus,
}
impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl artist::extension::host::Host for State {
    async fn run_command(
        &mut self,
        program: String,
        args: Vec<String>,
        stdin: Option<String>,
    ) -> std::result::Result<String, String> {
        let mut command = Command::new(program);
        command.args(args).kill_on_drop(true);
        if stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(input.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
        }
        let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }
    async fn spawn_command(
        &mut self,
        program: String,
        args: Vec<String>,
    ) -> std::result::Result<u64, String> {
        let result = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn();
        match result {
            Ok(child) => {
                self.next_child += 1;
                self.children.insert(self.next_child, child);
                Ok(self.next_child)
            }
            Err(e) => Err(e.to_string()),
        }
    }
    async fn process_write(&mut self, id: u64, data: String) -> std::result::Result<(), String> {
        let Some(child) = self.children.get_mut(&id) else {
            return Err("unknown process".into());
        };
        match child.stdin.as_mut() {
            Some(stdin) => stdin
                .write_all(data.as_bytes())
                .await
                .map_err(|e| e.to_string()),
            None => Err("process stdin closed".into()),
        }
    }
    async fn process_kill(&mut self, id: u64) -> std::result::Result<(), String> {
        let Some(mut child) = self.children.remove(&id) else {
            return Err("unknown process".into());
        };
        child.kill().await.map_err(|e| e.to_string())
    }
    async fn steer(&mut self, message: String) {
        self.control.steer(message).await;
    }
    async fn prompt_after(&mut self, message: String) {
        self.control.prompt_after(message).await;
    }
    async fn stop(&mut self) {
        self.control.stop().await;
    }
    async fn current_context(&mut self) -> String {
        serde_json::to_string(&*self.context.read().expect("extension context poisoned"))
            .unwrap_or_else(|_| "{}".into())
    }
    async fn recent_event_blocks(&mut self, count: u32) -> Vec<String> {
        let events = self.events.recent();
        let start = events.len().saturating_sub(count as usize);
        events[start..].to_vec()
    }
}

/// wasmtime 46's `Error` no longer implements `std::error::Error`; convert
/// explicitly at the API boundary.
fn wasm_err(error: wasmtime::Error) -> anyhow::Error {
    anyhow::anyhow!("{error:#}")
}

const EXT_EPOCH_TICK_MILLIS: u64 = 100;
/// Yield back to the async runtime after this many epoch ticks of uninterrupted
/// guest execution, then re-arm. Extensions keep every capability; this only
/// stops a spinning `.wasm` from hard-locking the harness — instead it yields
/// periodically and the runtime stays responsive.
const EXT_EPOCH_DEADLINE_TICKS: u64 = 1;

/// Shared engine for every extension, with epoch interruption enabled and one
/// background ticker driving all stores' deadlines. Mirrors the rules WASM tier.
fn extension_engine() -> &'static Engine {
    static ENGINE: std::sync::OnceLock<Engine> = std::sync::OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("extension wasmtime config");
        let ticker = engine.weak();
        std::thread::Builder::new()
            .name("artist-ext-epoch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(EXT_EPOCH_TICK_MILLIS));
                    let Some(engine) = ticker.upgrade() else {
                        return;
                    };
                    engine.increment_epoch();
                }
            })
            .expect("spawn extension epoch ticker");
        engine
    })
}

pub struct Instance {
    store: Mutex<Store<State>>,
    bindings: ArtistExtension,
}
impl Instance {
    pub async fn load(
        extension: &DiscoveredExtension,
        context: Arc<RwLock<ExtensionContext>>,
        events: EventBus,
        control: Arc<dyn HostControl>,
    ) -> Result<Self> {
        // async support is always on in wasmtime 46; the old toggle is gone.
        let engine = extension_engine();
        let component = Component::from_file(engine, &extension.wasm).map_err(wasm_err)?;
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(wasm_err)?;
        ArtistExtension::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s).map_err(wasm_err)?;
        let mut wasi = WasiCtx::builder();
        wasi.inherit_stdio().inherit_env().inherit_network();
        #[cfg(unix)]
        wasi.preopened_dir("/", "/", DirPerms::all(), FilePerms::all())
            .map_err(wasm_err)?;
        let mut store = Store::new(
            engine,
            State {
                wasi: wasi.build(),
                table: ResourceTable::new(),
                children: HashMap::new(),
                next_child: 0,
                control,
                context: context.clone(),
                events,
            },
        );
        // Stability guard: on each deadline the guest yields to the async
        // runtime and the deadline re-arms, so a spinning extension can't wedge
        // the harness. Capability is unchanged.
        store.set_epoch_deadline(EXT_EPOCH_DEADLINE_TICKS);
        store.epoch_deadline_async_yield_and_update(EXT_EPOCH_DEADLINE_TICKS);
        let bindings = ArtistExtension::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(wasm_err)?;
        let activation_context =
            serde_json::to_string(&*context.read().expect("extension context poisoned"))?;
        bindings
            .call_activate(&mut store, &activation_context)
            .await
            .map_err(wasm_err)?
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            store: Mutex::new(store),
            bindings,
        })
    }
    pub async fn invoke_tool(&self, name: &str, arguments: &serde_json::Value) -> Result<String> {
        let mut s = self.store.lock().await;
        self.bindings
            .call_invoke_tool(&mut *s, name, &arguments.to_string())
            .await
            .map_err(wasm_err)?
            .map_err(anyhow::Error::msg)
    }
    pub async fn invoke_command(&self, name: &str, arguments: &str) -> Result<String> {
        let mut s = self.store.lock().await;
        self.bindings
            .call_invoke_command(&mut *s, name, arguments)
            .await
            .map_err(wasm_err)?
            .map_err(anyhow::Error::msg)
    }
    pub async fn status(&self, name: &str) -> Result<String> {
        let mut s = self.store.lock().await;
        self.bindings
            .call_status(&mut *s, name)
            .await
            .map_err(wasm_err)?
            .map_err(anyhow::Error::msg)
    }
    pub async fn event(&self, json: &str) -> Result<()> {
        let mut s = self.store.lock().await;
        self.bindings
            .call_event(&mut *s, json)
            .await
            .map_err(wasm_err)?;
        Ok(())
    }
}
