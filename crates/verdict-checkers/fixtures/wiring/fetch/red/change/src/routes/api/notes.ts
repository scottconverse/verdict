import { createServerFileRoute } from "@tanstack/react-start/server";

export const ServerRoute = createServerFileRoute("/api/notes").methods({
  GET: listNotes,
});

async function listNotes() {
  return new Response(JSON.stringify([{ id: 1, text: "first" }]));
}
