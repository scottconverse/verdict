export function NotesPage() {
  async function save() {
    await fetch("/api/v1/notes", { method: "POST" });
  }
  return <button onClick={save}>Save note</button>;
}
