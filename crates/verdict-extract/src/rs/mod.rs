//! Rust extractors: Tauri `#[tauri::command]` handlers and the Rust definitions/calls pass.

use crate::Extractor;

/// Every Rust extractor.
pub fn extractors() -> Vec<Box<dyn Extractor>> {
    Vec::new()
}
