use crate::{DiscoveredExtension, ExtensionContext, HostControl};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};
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
}

pub struct Instance {
    store: Mutex<Store<State>>,
    bindings: ArtistExtension,
}
impl Instance {
    pub async fn load(
        extension: &DiscoveredExtension,
        context: &ExtensionContext,
        control: Arc<dyn HostControl>,
    ) -> Result<Self> {
        let mut config = Config::new();
        config.async_support(true).wasm_component_model(true);
        let engine = Engine::new(&config)?;
        let component = Component::from_file(&engine, &extension.wasm)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        ArtistExtension::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)?;
        let mut wasi = WasiCtx::builder();
        wasi.inherit_stdio().inherit_env().inherit_network();
        #[cfg(unix)]
        wasi.preopened_dir("/", "/", DirPerms::all(), FilePerms::all())?;
        let mut store = Store::new(
            &engine,
            State {
                wasi: wasi.build(),
                table: ResourceTable::new(),
                children: HashMap::new(),
                next_child: 0,
                control,
            },
        );
        let bindings = ArtistExtension::instantiate_async(&mut store, &component, &linker).await?;
        bindings
            .call_activate(&mut store, &serde_json::to_string(context)?)
            .await?
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
            .await?
            .map_err(anyhow::Error::msg)
    }
    pub async fn invoke_command(&self, name: &str, arguments: &str) -> Result<String> {
        let mut s = self.store.lock().await;
        self.bindings
            .call_invoke_command(&mut *s, name, arguments)
            .await?
            .map_err(anyhow::Error::msg)
    }
    pub async fn status(&self, name: &str) -> Result<String> {
        let mut s = self.store.lock().await;
        self.bindings
            .call_status(&mut *s, name)
            .await?
            .map_err(anyhow::Error::msg)
    }
    pub async fn event(&self, json: &str) -> Result<()> {
        let mut s = self.store.lock().await;
        self.bindings.call_event(&mut *s, json).await?;
        Ok(())
    }
}
