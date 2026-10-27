import { createServerFn } from "@tanstack/react-start";
import { db } from "./db";

export const listNotes = createServerFn({ method: "GET" }).handler(async () => {
  throw new Error("not implemented");
});
