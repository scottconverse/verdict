import { createServerFileRoute } from "@tanstack/react-start/server";

export const ServerRoute = createServerFileRoute("/api/notes").methods({
  GET: listNotes,
  POST: createNote,
});

async function listNotes() {
  return new Response(JSON.stringify([{ id: 1, text: "first" }]));
}

async function createNote({ request }: { request: Request }) {
  const note = await request.json();
  return new Response(JSON.stringify(note), { status: 201 });
}
