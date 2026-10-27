import { createServerFn } from "@tanstack/react-start";
import { db } from "./db";

export const listNotes = createServerFn({ method: "GET" }).handler(async () => {
  const rows = await db.selectFrom("notes").selectAll().execute();
  return rows;
});
