import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";

const server = new McpServer({ name: "notes", version: "1.0.0" });

server.registerTool("search", { description: "Search the notes" }, searchNotes);

export default server;
