//! Python extractors: FastAPI, SQLAlchemy, asyncpg, Alembic, Typer, Python MCP tools, and the
//! Python definitions/calls/imports pass.

use crate::Extractor;

/// Every Python extractor.
pub fn extractors() -> Vec<Box<dyn Extractor>> {
    Vec::new()
}
