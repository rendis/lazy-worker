use super::App;
use std::{collections::HashSet, path::Path};

pub fn supported() -> Result<(), &'static str> {
    Err("unsupported")
}

pub fn scan() -> Vec<App> {
    Vec::new()
}

pub fn running() -> HashSet<String> {
    HashSet::new()
}

pub fn render_icon(_: &Path) -> Result<Vec<u8>, &'static str> {
    Err("unsupported")
}
