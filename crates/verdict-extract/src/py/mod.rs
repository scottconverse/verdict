//! Python extractors: FastAPI, SQLAlchemy, asyncpg, Alembic, Typer, Python MCP tools, and the
//! Python definitions/calls/imports pass.
//!
//! Everything here reads source text with tree-sitter. Nothing imports or runs the user's Python.

mod alembic;
mod asyncpg;
mod common;
mod fastapi;
mod graph;
mod mcp;
mod sql;
mod sqlalchemy;
mod symbols;
mod typer;

use crate::Extractor;

/// Every Python extractor.
pub fn extractors() -> Vec<Box<dyn Extractor>> {
    vec![
        Box::new(symbols::PySymbols),
        Box::new(fastapi::FastApi),
        Box::new(sqlalchemy::SqlAlchemy),
        Box::new(asyncpg::Asyncpg),
        Box::new(alembic::Alembic),
        Box::new(typer::TyperCli),
        Box::new(mcp::McpPython),
    ]
}
