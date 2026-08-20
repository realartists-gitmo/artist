//! Host binding for the generic verb registry imported by extensions.

use std::sync::Arc;

use artist_kernel::{VerbInvocationError, VerbRegistry};
use wasmtime::component::{HasData, Linker};

mod bindings {
    wasmtime::component::bindgen!({
        path: "verbs/wit",
        world: "verbs-extension",
        imports: { default: async | trappable },
    });
}

pub struct VerbRegistryContext<'a> {
    pub registry: Arc<VerbRegistry>,
    _borrow: std::marker::PhantomData<&'a mut ()>,
}

impl<'a> VerbRegistryContext<'a> {
    pub fn new(registry: Arc<VerbRegistry>) -> Self {
        Self {
            registry,
            _borrow: std::marker::PhantomData,
        }
    }
}

pub trait VerbRegistryView: Send {
    fn verb_registry(&mut self) -> VerbRegistryContext<'_>;
}

pub struct RegistryHost;

impl HasData for RegistryHost {
    type Data<'a> = VerbRegistryContext<'a>;
}

impl From<VerbInvocationError> for bindings::artist::verbs::registry::Error {
    fn from(error: VerbInvocationError) -> Self {
        use bindings::artist::verbs::registry::Error;
        match error {
            VerbInvocationError::InvalidArgument(_) => Error::InvalidArgument,
            VerbInvocationError::NotFound(_) => Error::NotFound,
            VerbInvocationError::Unsupported(_) => Error::Unsupported,
            VerbInvocationError::PermissionDenied(_) => Error::PermissionDenied,
            VerbInvocationError::Conflict(_) => Error::Conflict,
            VerbInvocationError::Aborted(_) => Error::Aborted,
            VerbInvocationError::Internal(_) => Error::Internal,
        }
    }
}

impl bindings::artist::verbs::registry::Host for VerbRegistryContext<'_> {
    async fn invoke(
        &mut self,
        name: String,
        request: Vec<u8>,
    ) -> wasmtime::Result<Result<Vec<u8>, bindings::artist::verbs::registry::Error>> {
        Ok(self
            .registry
            .invoke(&name, &request)
            .await
            .map_err(bindings::artist::verbs::registry::Error::from)
            .map(Ok)?)
    }
}

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: VerbRegistryView + 'static,
{
    bindings::artist::verbs::registry::add_to_linker::<_, RegistryHost>(linker, T::verb_registry)
}
