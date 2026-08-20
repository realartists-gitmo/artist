//! Generic component-master bindings and host projection.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::runtime::RuntimeStore;

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "component-root",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

pub use bindings::artist::component::{host, types};

#[async_trait]
pub trait ComponentLoader: Send + Sync {
    async fn load(&self, locator: &str) -> anyhow::Result<ComponentRecord>;
    async fn describe(&self, component: &ComponentRecord) -> anyhow::Result<types::Descriptor>;
    async fn activate(&self, component: &ComponentRecord) -> anyhow::Result<ComponentRecord>;
    async fn retire(&self, component: &ComponentRecord) -> anyhow::Result<()>;
    async fn register(
        &self,
        component: &ComponentRecord,
        uri: &str,
    ) -> anyhow::Result<RegistrationRecord>;
    async fn unregister(&self, registration: &RegistrationRecord) -> anyhow::Result<()>;
}

#[derive(Clone, Debug)]
pub struct ComponentRecord {
    pub id: u64,
    pub descriptor: types::Descriptor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationRecord {
    pub id: u64,
}

pub struct UnavailableComponentLoader;

#[async_trait]
impl ComponentLoader for UnavailableComponentLoader {
    async fn load(&self, _locator: &str) -> anyhow::Result<ComponentRecord> {
        Err(anyhow!("component loader is not installed"))
    }
    async fn describe(&self, _component: &ComponentRecord) -> anyhow::Result<types::Descriptor> {
        Err(anyhow!("component loader is not installed"))
    }
    async fn activate(&self, _component: &ComponentRecord) -> anyhow::Result<ComponentRecord> {
        Err(anyhow!("component loader is not installed"))
    }
    async fn retire(&self, _component: &ComponentRecord) -> anyhow::Result<()> {
        Err(anyhow!("component loader is not installed"))
    }
    async fn register(
        &self,
        _component: &ComponentRecord,
        _uri: &str,
    ) -> anyhow::Result<RegistrationRecord> {
        Err(anyhow!("component loader is not installed"))
    }
    async fn unregister(&self, _registration: &RegistrationRecord) -> anyhow::Result<()> {
        Err(anyhow!("component loader is not installed"))
    }
}

pub(crate) struct MasterState {
    components: BTreeMap<u32, ComponentRecord>,
    registrations: BTreeMap<u32, RegistrationRecord>,
    next_component: u32,
    next_registration: u32,
}

impl Default for MasterState {
    fn default() -> Self {
        Self {
            components: BTreeMap::new(),
            registrations: BTreeMap::new(),
            next_component: 1,
            next_registration: 1,
        }
    }
}

#[derive(Clone)]
pub struct MasterContext {
    pub loader: Arc<dyn ComponentLoader>,
    state: Arc<Mutex<MasterState>>,
}

impl MasterContext {
    pub fn new(loader: Arc<dyn ComponentLoader>) -> Self {
        Self::with_state(loader, Arc::new(Mutex::new(MasterState::default())))
    }

    pub(crate) fn with_state(
        loader: Arc<dyn ComponentLoader>,
        state: Arc<Mutex<MasterState>>,
    ) -> Self {
        Self { loader, state }
    }

    fn insert_component(&self, component: ComponentRecord) -> Resource<types::Component> {
        let mut state = self.state.lock().unwrap();
        let id = state.next_component;
        state.next_component += 1;
        state.components.insert(id, component);
        Resource::new_own(id)
    }

    fn component(&self, resource: Resource<types::Component>) -> anyhow::Result<ComponentRecord> {
        self.state
            .lock()
            .unwrap()
            .components
            .get(&resource.rep())
            .cloned()
            .ok_or_else(|| anyhow!("unknown component handle"))
    }
}

pub trait MasterView: Send {
    fn master(&mut self) -> MasterContext;
}

pub struct MasterHost;

impl wasmtime::component::HasData for MasterHost {
    type Data<'a> = MasterContext;
}

impl MasterView for RuntimeStore {
    fn master(&mut self) -> MasterContext {
        MasterContext::with_state(
            Arc::clone(&self.component_loader),
            Arc::clone(&self.master_state),
        )
    }
}

fn master_error(_error: anyhow::Error) -> types::Error {
    types::Error::Failed
}

impl types::HostComponent for MasterContext {
    async fn drop(&mut self, rep: Resource<types::Component>) -> wasmtime::Result<()> {
        self.state.lock().unwrap().components.remove(&rep.rep());
        Ok(())
    }
}

impl types::HostRegistration for MasterContext {
    async fn drop(&mut self, rep: Resource<types::Registration>) -> wasmtime::Result<()> {
        self.state.lock().unwrap().registrations.remove(&rep.rep());
        Ok(())
    }
}

impl types::Host for MasterContext {}

impl host::Host for MasterContext {
    async fn load(
        &mut self,
        locator: String,
    ) -> wasmtime::Result<Result<Resource<types::Component>, types::Error>> {
        match self.loader.load(&locator).await {
            Ok(value) => Ok(Ok(self.insert_component(value))),
            Err(error) => Ok(Err(master_error(error))),
        }
    }
    async fn describe(
        &mut self,
        value: Resource<types::Component>,
    ) -> wasmtime::Result<Result<types::Descriptor, types::Error>> {
        let component = self.component(value).map_err(wasmtime::Error::msg)?;
        match self.loader.describe(&component).await {
            Ok(value) => Ok(Ok(value)),
            Err(error) => Ok(Err(master_error(error))),
        }
    }
    async fn activate(
        &mut self,
        value: Resource<types::Component>,
    ) -> wasmtime::Result<Result<Resource<types::Component>, types::Error>> {
        let component = self.component(value).map_err(wasmtime::Error::msg)?;
        match self.loader.activate(&component).await {
            Ok(value) => Ok(Ok(self.insert_component(value))),
            Err(error) => Ok(Err(master_error(error))),
        }
    }
    async fn retire(
        &mut self,
        value: Resource<types::Component>,
    ) -> wasmtime::Result<Result<(), types::Error>> {
        let component = self.component(value).map_err(wasmtime::Error::msg)?;
        match self.loader.retire(&component).await {
            Ok(()) => Ok(Ok(())),
            Err(error) => Ok(Err(master_error(error))),
        }
    }
    async fn register(
        &mut self,
        value: Resource<types::Component>,
        uri: String,
    ) -> wasmtime::Result<Result<Resource<types::Registration>, types::Error>> {
        let component = self.component(value).map_err(wasmtime::Error::msg)?;
        match self.loader.register(&component, &uri).await {
            Ok(registration) => {
                let mut state = self.state.lock().unwrap();
                let id = state.next_registration;
                state.next_registration += 1;
                state.registrations.insert(id, registration);
                Ok(Ok(Resource::new_own(id)))
            }
            Err(error) => Ok(Err(master_error(error))),
        }
    }
    async fn unregister(
        &mut self,
        value: Resource<types::Registration>,
    ) -> wasmtime::Result<Result<(), types::Error>> {
        let registration = self
            .state
            .lock()
            .unwrap()
            .registrations
            .remove(&value.rep())
            .ok_or_else(|| wasmtime::Error::msg("unknown registration handle"))?;
        match self.loader.unregister(&registration).await {
            Ok(()) => Ok(Ok(())),
            Err(error) => Ok(Err(master_error(error))),
        }
    }
}

pub fn add_to_linker<T>(linker: &mut wasmtime::component::Linker<T>) -> wasmtime::Result<()>
where
    T: MasterView + 'static,
{
    bindings::ComponentRoot::add_to_linker::<T, MasterHost>(linker, T::master)
}
