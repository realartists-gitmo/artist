use anyhow::Result;
use dialoguer::{Select, theme::ColorfulTheme};

pub fn select(label: &str, items: &[String], default: usize) -> Result<usize> {
    Ok(Select::with_theme(&ColorfulTheme::default())
        .with_prompt(label)
        .items(items)
        .default(default)
        .interact()?)
}
