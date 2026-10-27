export function NotesPage() {
  async function save() {
    await fetch("/api/notes", { method: "POST" });
  }
  return <button onClick={save}>Save note</button>;
}
