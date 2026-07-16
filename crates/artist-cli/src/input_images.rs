use std::{path::PathBuf, sync::Arc};

const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug)]
struct TempImage(PathBuf);

impl Drop for TempImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ImagePaste {
    pub path: PathBuf,
    pub media_type: ImageKind,
    _temporary: Option<Arc<TempImage>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

pub(crate) fn image_paste(value: &str) -> Option<ImagePaste> {
    let path = PathBuf::from(value.trim()).canonicalize().ok()?;
    let temp = std::env::temp_dir().canonicalize().ok()?;
    let trusted_clipboard = path.parent().is_some_and(|parent| {
        parent.parent() == Some(temp.as_path())
            && parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("herdr-clipboard-images-"))
    });
    let artist_clipboard = path.parent() == Some(temp.as_path())
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("artist-paste-"));
    if (!trusted_clipboard && !artist_clipboard)
        || std::fs::metadata(&path).ok()?.len() > MAX_IMAGE_BYTES
    {
        return None;
    }
    let media_type = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => ImageKind::Png,
        "jpg" | "jpeg" => ImageKind::Jpeg,
        "gif" => ImageKind::Gif,
        "webp" => ImageKind::Webp,
        _ => return None,
    };
    let temporary = artist_clipboard.then(|| Arc::new(TempImage(path.clone())));
    Some(ImagePaste {
        path,
        media_type,
        _temporary: temporary,
    })
}
