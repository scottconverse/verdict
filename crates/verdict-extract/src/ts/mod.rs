//! TypeScript and JavaScript extractors: fetch, openapi-typescript clients, TanStack Start server
//! functions and routes, Kysely, TS MCP tools, Tauri `invoke`, React buttons/forms/links, and the
//! TS/JS definitions/calls/imports pass.

use crate::Extractor;

/// Every TypeScript/JavaScript extractor.
pub fn extractors() -> Vec<Box<dyn Extractor>> {
    Vec::new()
}
