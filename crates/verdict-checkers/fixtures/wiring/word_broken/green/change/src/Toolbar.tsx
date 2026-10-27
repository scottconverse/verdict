export function Toolbar() {
  return <button onClick={exportNotes}>Export</button>;
}

function exportNotes() {
  const blob = new Blob(["notes"], { type: "text/plain" });
  return URL.createObjectURL(blob);
}
