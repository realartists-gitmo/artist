//! Small picker retained for authentication setup.

use anyhow::Result;

pub fn pick(
    _label: &str,
    items: &[String],
    default: usize,
    _draw: &mut impl FnMut(&[String]) -> Result<()>,
) -> Result<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }
    Ok(Some(default.min(items.len() - 1)))
}
