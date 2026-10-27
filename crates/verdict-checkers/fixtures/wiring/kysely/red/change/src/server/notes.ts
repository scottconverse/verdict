import { createServerFn } from "@tanstack/react-start";
import { db } from "./db";

export const saveNote = createServerFn({ method: "POST" }).handler(async ({ data }) => {
  console.log("note received", data);
  return { ok: true };
});

export async function archiveNote(id: number) {
  await db.insertInto("archive").values({ note_id: id }).execute();
}
