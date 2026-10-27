import { createServerFileRoute } from "@tanstack/react-start/server";

export const ServerRoute = createServerFileRoute("/users/$id").methods({
  GET: getUser,
});

async function getUser({ params }: { params: { id: string } }) {
  return new Response(JSON.stringify({ id: params.id, name: "Ada" }));
}
