use anyhow::Result;
use dialoguer::{Input, Password, Select, theme::ColorfulTheme};

pub fn text(label: &str, default: Option<&str>) -> Result<String> {
    let theme = ColorfulTheme::default();
    let mut prompt = Input::<String>::with_theme(&theme);
    prompt = prompt.with_prompt(label);
    if let Some(value) = default {
        prompt = prompt.default(value.to_owned());
    }
    Ok(prompt.interact_text()?)
}
pub fn secret(label: &str) -> Result<String> {
    Ok(Password::with_theme(&ColorfulTheme::default())
        .with_prompt(label)
        .interact()?)
}
pub fn select(label: &str, items: &[String], default: usize) -> Result<usize> {
    Ok(Select::with_theme(&ColorfulTheme::default())
        .with_prompt(label)
        .items(items)
        .default(default)
        .interact()?)
}
