from mcp.server.fastmcp import FastMCP

mcp = FastMCP("notes")
notes: dict[int, str] = {}


@mcp.tool()
def list_notes() -> list[str]:
    return list(notes.values())


@mcp.tool()
def delete_note(note_id: int) -> bool:
    return notes.pop(note_id, None) is not None
