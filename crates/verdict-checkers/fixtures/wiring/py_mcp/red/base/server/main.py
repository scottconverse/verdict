from mcp.server.fastmcp import FastMCP

mcp = FastMCP("notes")
notes: dict[int, str] = {}


@mcp.tool()
def list_notes() -> list[str]:
    return list(notes.values())
