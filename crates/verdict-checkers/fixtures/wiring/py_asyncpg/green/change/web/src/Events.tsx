export function Events() {
  async function record() {
    await fetch("/api/events", { method: "POST" });
  }
  return <button onClick={record}>Record event</button>;
}
