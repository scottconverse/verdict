import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/notes")({ component: NotesPage });

function NotesPage() {
  return (
    <ul>
      <li>First note</li>
    </ul>
  );
}
