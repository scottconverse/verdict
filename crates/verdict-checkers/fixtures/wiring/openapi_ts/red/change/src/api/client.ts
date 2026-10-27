import createClient from "openapi-fetch";
import type { paths } from "./schema";

const client = createClient<paths>({ baseUrl: "/" });

export function loadUser(id: string) {
  return client.GET("/user/{id}", { params: { path: { id } } });
}
