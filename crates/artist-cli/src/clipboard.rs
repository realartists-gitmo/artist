use crate::{
    chat_ui::{ChatInput, SubmittedPrompt},
    input_images::ImageKind,
};
use anyhow::{Context, Result};

pub(crate) fn paste(input: &mut ChatInput, allow_image: bool) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("open clipboard")?;
    if allow_image && let Ok(image) = clipboard.get_image() {
        let path = std::env::temp_dir().join(format!(
            "artist-paste-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        image::save_buffer(
            &path,
            image.bytes.as_ref(),
            image.width as u32,
            image.height as u32,
            image::ColorType::Rgba8,
        )
        .with_context(|| format!("save clipboard image {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        input.paste(path.to_string_lossy().as_ref(), true);
    } else if let Ok(text) = clipboard.get_text() {
        input.paste(&text, allow_image);
    }
    Ok(())
}

pub(crate) fn agent_input(prompt: &SubmittedPrompt) -> Result<artist_agent::ChatInput> {
    anyhow::ensure!(
        prompt.images.len() <= 4,
        "at most four pasted images are allowed"
    );
    let images = prompt
        .images
        .iter()
        .map(|image| {
            let data = std::fs::read(&image.path)
                .with_context(|| format!("read pasted image {}", image.path.display()))?;
            anyhow::ensure!(data.len() <= 10 * 1024 * 1024, "pasted image is too large");
            let expected = match image.media_type {
                ImageKind::Png => image::ImageFormat::Png,
                ImageKind::Jpeg => image::ImageFormat::Jpeg,
                ImageKind::Gif => image::ImageFormat::Gif,
                ImageKind::Webp => image::ImageFormat::WebP,
            };
            let mut reader = image::ImageReader::new(std::io::Cursor::new(&data))
                .with_guessed_format()
                .context("detect pasted image format")?;
            anyhow::ensure!(
                reader.format() == Some(expected),
                "pasted image format does not match its extension"
            );
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            reader.decode().context("decode pasted image")?;
            Ok(match image.media_type {
                ImageKind::Png => artist_agent::ImageAttachment::png(data),
                ImageKind::Jpeg => artist_agent::ImageAttachment::jpeg(data),
                ImageKind::Gif => artist_agent::ImageAttachment::gif(data),
                ImageKind::Webp => artist_agent::ImageAttachment::webp(data),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(artist_agent::ChatInput {
        text: prompt.content.clone(),
        images,
    })
}
