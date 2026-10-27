export async function sendNote(text: string) {
  const res = await fetch("/api/notes", { method: "POST", body: JSON.stringify({ text }) });
  return res.ok;
}
