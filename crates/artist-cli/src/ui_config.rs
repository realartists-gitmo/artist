use anyhow::{Context, Result, bail};
use ratatui::style::Color;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};
use toml::{Table, Value};

const DEFAULT_RGB: u32 = 0x00ff_ffff;
static COLOR_SCHEME: AtomicU32 = AtomicU32::new(DEFAULT_RGB);

pub(crate) fn load(config_root: &Path) -> Result<()> {
    let path = path(config_root);
    if !path.exists() {
        COLOR_SCHEME.store(DEFAULT_RGB, Ordering::Relaxed);
        return Ok(());
    }
    let config = read_table(&path)?;
    let rgb = config
        .get("color_scheme")
        .and_then(Value::as_str)
        .map(parse_hex)
        .transpose()?
        .unwrap_or(DEFAULT_RGB);
    COLOR_SCHEME.store(rgb, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn save_color(config_root: &Path, value: &str) -> Result<Color> {
    let rgb = parse_hex(value)?;
    let path = path(config_root);
    let mut config = if path.exists() {
        read_table(&path)?
    } else {
        Table::new()
    };
    config.insert(
        "color_scheme".to_owned(),
        Value::String(format!("#{rgb:06x}")),
    );
    fs::create_dir_all(config_root)?;
    fs::write(&path, toml::to_string_pretty(&config)?)
        .with_context(|| format!("write UI config {}", path.display()))?;
    COLOR_SCHEME.store(rgb, Ordering::Relaxed);
    Ok(to_color(rgb))
}

pub(crate) fn color() -> Color {
    to_color(COLOR_SCHEME.load(Ordering::Relaxed))
}

pub(crate) fn rgb() -> (u8, u8, u8) {
    unpack_rgb(COLOR_SCHEME.load(Ordering::Relaxed))
}

pub(crate) fn contrast_color() -> Color {
    contrast_for(rgb())
}

fn read_table(path: &Path) -> Result<Table> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("read UI config {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse UI config {}", path.display()))
}

fn path(config_root: &Path) -> PathBuf {
    config_root.join("config.toml")
}

fn parse_hex(value: &str) -> Result<u32> {
    let Some(hex) = value.strip_prefix('#') else {
        bail!("color must be a 6-digit hex value such as #ffffff");
    };
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("color must be a 6-digit hex value such as #ffffff");
    }
    u32::from_str_radix(hex, 16).context("parse color hex value")
}

fn unpack_rgb(value: u32) -> (u8, u8, u8) {
    ((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn to_color(value: u32) -> Color {
    let (red, green, blue) = unpack_rgb(value);
    Color::Rgb(red, green, blue)
}

fn contrast_for((red, green, blue): (u8, u8, u8)) -> Color {
    let luminance = u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114;
    if luminance >= 128_000 {
        Color::Black
    } else {
        Color::White
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_color_without_removing_other_config() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("config.toml"),
            "other = true\n[future]\nvalue = 7\n",
        )
        .unwrap();
        save_color(root.path(), "#12Abef").unwrap();
        COLOR_SCHEME.store(DEFAULT_RGB, Ordering::Relaxed);

        let saved = read_table(&root.path().join("config.toml")).unwrap();
        assert_eq!(saved["color_scheme"].as_str(), Some("#12abef"));
        assert_eq!(saved["other"].as_bool(), Some(true));
        assert_eq!(saved["future"]["value"].as_integer(), Some(7));
    }

    #[test]
    fn rejects_invalid_colors() {
        assert!(parse_hex("ffffff").is_err());
        assert!(parse_hex("#fff").is_err());
        assert!(parse_hex("#fffffg").is_err());
    }

    #[test]
    fn chooses_readable_highlight_text() {
        assert_eq!(contrast_for((255, 255, 255)), Color::Black);
        assert_eq!(contrast_for((0, 20, 60)), Color::White);
    }
}
