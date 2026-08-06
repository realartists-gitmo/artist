use std::{ffi::OsString, path::PathBuf, sync::Arc};

pub const AGENT: &str = "artist";
pub const LIFECYCLE_SOURCE: &str = "artist:extension";

#[derive(Clone, Debug)]
pub struct HerdrContext {
    pub(crate) pane_id: Arc<str>,
    pub(crate) binary: Arc<PathBuf>,
    pub(crate) socket_path: Option<Arc<PathBuf>>,
}

impl HerdrContext {
    pub fn detect() -> Option<Self> {
        Self::detect_with(|name| std::env::var_os(name))
    }

    fn detect_with(mut get: impl FnMut(&str) -> Option<OsString>) -> Option<Self> {
        if get("HERDR_ENV").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return None;
        }
        let pane_id = get("HERDR_PANE_ID")?.into_string().ok()?;
        if pane_id.is_empty() {
            return None;
        }
        let binary = get("HERDR_BIN_PATH")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("herdr"));
        let socket_path = get("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(Arc::new);
        Some(Self {
            pane_id: pane_id.into(),
            binary: Arc::new(binary),
            socket_path,
        })
    }

    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }

    pub fn binary(&self) -> &std::path::Path {
        &self.binary
    }

    pub fn socket_path(&self) -> Option<&std::path::Path> {
        self.socket_path.as_deref().map(PathBuf::as_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn detect(values: &[(&str, &str)]) -> Option<HerdrContext> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        HerdrContext::detect_with(|name| values.get(name).cloned())
    }

    #[test]
    fn activation_is_strictly_environment_gated() {
        assert!(detect(&[("HERDR_PANE_ID", "1-2")]).is_none());
        assert!(detect(&[("HERDR_ENV", "0"), ("HERDR_PANE_ID", "1-2")]).is_none());
        assert!(detect(&[("HERDR_ENV", "1")]).is_none());
        assert!(detect(&[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "")]).is_none());
    }

    #[test]
    fn detects_cli_and_optional_socket() {
        let context = detect(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "2-3"),
            ("HERDR_BIN_PATH", "/opt/herdr"),
            ("HERDR_SOCKET_PATH", "/tmp/herdr.sock"),
        ])
        .unwrap();
        assert_eq!(context.pane_id(), "2-3");
        assert_eq!(context.binary(), std::path::Path::new("/opt/herdr"));
        assert_eq!(
            context.socket_path(),
            Some(std::path::Path::new("/tmp/herdr.sock"))
        );
    }

    #[test]
    fn falls_back_to_path_lookup() {
        let context = detect(&[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "1-1")]).unwrap();
        assert_eq!(context.binary(), std::path::Path::new("herdr"));
    }
}
