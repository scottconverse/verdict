export async function loadNotes() {
  const r = await fetch("/api/notes");
  return r.json();
}
