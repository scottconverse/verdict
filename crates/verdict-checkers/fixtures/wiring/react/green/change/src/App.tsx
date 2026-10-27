export function App() {
  return (
    <main>
      <h1>Notes</h1>
      <button onClick={archiveAll}>Archive</button>
    </main>
  );
}

function archiveAll() {
  window.localStorage.setItem("archived", "yes");
}
