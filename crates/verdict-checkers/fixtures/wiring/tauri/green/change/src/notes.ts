import { invoke } from "@tauri-apps/api/core";

export async function storeNote(text: string) {
  await invoke("save_note", { text });
}
