use anyhow::Result;
use dialoguer::{Select, theme::ColorfulTheme};

pub fn select(label: &str, items: &[String], default: usize) -> Result<usize> {
    select_with_page_size(label, items, default, None)
}

pub fn select_paged(
    label: &str,
    items: &[String],
    default: usize,
    page_size: usize,
) -> Result<usize> {
    select_with_page_size(label, items, default, Some(page_size))
}

fn select_with_page_size(
    label: &str,
    items: &[String],
    default: usize,
    page_size: Option<usize>,
) -> Result<usize> {
    let theme = ColorfulTheme::default();
    let select = Select::with_theme(&theme)
        .with_prompt(label)
        .items(items)
        .default(default);
    let select = match page_size {
        Some(page_size) => select.max_length(page_size),
        None => select,
    };
    Ok(select.interact()?)
}
