//! TS/JS extractors on small sources: what each framework reader finds.

use verdict_extract::*;

fn facts(path: &str, src: &str) -> FileFacts {
    let lang = syntax::lang_of(path).expect("a TS/JS path");
    extract_source(path, src, lang, &registry())
}

fn tree(files: &[(&str, &str)]) -> Facts {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (p, text) in files {
        let full = dir.path().join(p);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
        paths.push(p.to_string());
    }
    extract_files(dir.path(), &paths, &ScanOptions::default(), &registry())
}

fn call_names(f: &FileFacts) -> Vec<(EndpointKind, Option<String>, String, bool)> {
    f.calls
        .iter()
        .map(|c| {
            (
                c.target.kind,
                c.target.method.clone(),
                c.target.name.clone(),
                c.dynamic,
            )
        })
        .collect()
}

#[test]
fn ts_fetch_and_axios_calls() {
    let f = facts(
        "src/api.ts",
        r#"
export async function load(id: string) {
  await fetch(`${API_BASE}/api/notes/${id}`);
  await fetch("/api/notes", { method: "post", body: "{}" });
  await fetch("https://example.com/elsewhere");
  await axios.get("/api/users");
  await api.delete(`/api/users/${id}`);
  cache.get("/not/a/client");
  router.get("/api/route", (req, res) => res.send(1));
}
"#,
    );
    assert_eq!(
        call_names(&f),
        [
            (EndpointKind::Http, Some("GET".into()), "/api/notes/{}".into(), true),
            (EndpointKind::Http, Some("POST".into()), "/api/notes".into(), false),
            (EndpointKind::Http, Some("GET".into()), "/api/users".into(), false),
            (EndpointKind::Http, Some("DELETE".into()), "/api/users/{}".into(), true),
        ]
    );
    assert!(f.calls.iter().all(|c| c.enclosing.as_deref() == Some("load")));
    assert!(f.frameworks.contains(&Framework::Fetch));
}

#[test]
fn ts_openapi_client_calls_and_schema_routes() {
    let f = facts(
        "src/client.ts",
        r#"
export interface paths {
  "/users/{id}": { parameters: {}; get: operations["getUser"]; put?: never; delete: operations["del"] };
}
export function show(id: string) { return client.GET("/users/{id}", { params: { path: { id } } }); }
"#,
    );
    assert_eq!(
        call_names(&f),
        [(EndpointKind::Http, Some("GET".into()), "/users/{}".into(), false)]
    );
    let routes: Vec<(Option<String>, String)> = f
        .endpoints
        .iter()
        .map(|e| (e.key.method.clone(), e.key.name.clone()))
        .collect();
    assert_eq!(
        routes,
        [
            (Some("GET".into()), "/users/{}".into()),
            (Some("DELETE".into()), "/users/{}".into())
        ]
    );
    assert!(f.frameworks.contains(&Framework::OpenapiTs));
}

#[test]
fn ts_tanstack_start_server_fns_and_routes() {
    let facts = tree(&[
        (
            "src/server/notes.ts",
            r#"
import { createServerFn } from "@tanstack/react-start";
export const saveNote = createServerFn({ method: "POST" }).handler(async ({ data }) => {
  await db.insertInto("notes").values(data).execute();
});
export const listNotes = createServerFn().handler(async () => { throw new Error("not implemented"); });
"#,
        ),
        (
            "src/routes/api/notes.ts",
            r#"
export const ServerRoute = createServerFileRoute("/api/notes").methods({
  GET: async () => new Response("[]"),
  POST: createNote,
});
"#,
        ),
        (
            "src/components/Form.tsx",
            r#"
import { saveNote } from "../server/notes";
export function NoteForm() {
  const save = useServerFn(saveNote);
  return <button onClick={() => saveNote({ data: {} })}>Save</button>;
}
"#,
        ),
    ]);
    let eps: Vec<(EndpointKind, Option<String>, String, Option<String>)> = facts
        .endpoints()
        .map(|e| {
            (
                e.key.kind,
                e.key.method.clone(),
                e.key.name.clone(),
                e.handler.clone(),
            )
        })
        .collect();
    assert!(eps.contains(&(
        EndpointKind::Http,
        Some("GET".into()),
        "/api/notes".into(),
        Some("GET /api/notes".into())
    )));
    assert!(eps.contains(&(
        EndpointKind::Http,
        Some("POST".into()),
        "/api/notes".into(),
        Some("createNote".into())
    )));
    assert!(eps.contains(&(
        EndpointKind::ServerFn,
        None,
        "saveNote".into(),
        Some("saveNote".into())
    )));
    // Two uses of saveNote in the component (useServerFn and the direct call) are calls.
    let calls: Vec<&Call> = facts
        .calls()
        .filter(|c| c.target.kind == EndpointKind::ServerFn)
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|c| c.target.name == "saveNote"));
    // The server fn's handler body is its definition; the write sits in it.
    let w: Vec<&DataWrite> = facts.writes().collect();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].enclosing.as_deref(), Some("saveNote"));
    let stub = facts.defs().find(|d| d.name == "listNotes").unwrap();
    assert!(stub.is_stub);
    let inline = facts.defs().find(|d| d.name == "GET /api/notes").unwrap();
    assert!(!inline.is_stub);
}

#[test]
fn ts_tanstack_router_pages() {
    let f = facts(
        "src/routes/notes/$id.tsx",
        r#"
export const Route = createFileRoute("/notes/$id")({ component: NotePage });
function NotePage() { return <div /> }
"#,
    );
    assert_eq!(f.pages.len(), 1);
    assert_eq!(f.pages[0].route, "/notes/{}");
    assert_eq!(f.pages[0].component.as_deref(), Some("NotePage"));
    let g = facts(
        "src/routes/about.tsx",
        "export const Route = createFileRoute()({ component: About });\n",
    );
    assert_eq!(g.pages[0].route, "/about");
    let api = facts(
        "src/routes/api/x.ts",
        "export const Route = createFileRoute('/api/x')({ server: { handlers: { GET: getX } } });\n",
    );
    assert!(api.pages.is_empty());
    assert_eq!(api.endpoints.len(), 1);
    assert_eq!(api.endpoints[0].handler.as_deref(), Some("getX"));
}

#[test]
fn ts_kysely_writes() {
    let f = facts(
        "src/db.ts",
        r#"
export async function changes(db: Kysely<DB>) {
  await db.insertInto("person").values({ name: "a" }).execute();
  await db.updateTable("person as p").set({ name: "b" }).execute();
  await db.deleteFrom("pet").where("id", "=", 1).execute();
  await db.replaceInto("tag").values({}).execute();
  await db.selectFrom("person").selectAll().execute();
}
"#,
    );
    let w: Vec<(WriteOp, Option<String>)> = f
        .writes
        .iter()
        .map(|w| (w.op, w.table.clone()))
        .collect();
    assert_eq!(
        w,
        [
            (WriteOp::Insert, Some("person".into())),
            (WriteOp::Update, Some("person".into())),
            (WriteOp::Delete, Some("pet".into())),
            (WriteOp::Upsert, Some("tag".into())),
        ]
    );
    assert!(f.writes.iter().all(|w| w.enclosing.as_deref() == Some("changes")));
}

#[test]
fn ts_mcp_tools() {
    let f = facts(
        "src/mcp.ts",
        r#"
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
server.tool("add", "Add two numbers", { a: z.number() }, async ({ a }) => ({ content: [] }));
server.registerTool("echo", { description: "Echo" }, echoHandler);
const listing = { tools: [{ name: "search", description: "Search", inputSchema: {} }] };
"#,
    );
    let tools: Vec<(String, Option<String>)> = f
        .endpoints
        .iter()
        .map(|e| (e.key.name.clone(), e.handler.clone()))
        .collect();
    assert_eq!(
        tools,
        [
            ("add".into(), Some("tool:add".into())),
            ("echo".into(), Some("echoHandler".into())),
            ("search".into(), None),
        ]
    );
    assert!(f.defs.iter().any(|d| d.name == "tool:add"));
}

#[test]
fn ts_tauri_invoke() {
    let f = facts(
        "src/notes.ts",
        r#"
import { invoke } from "@tauri-apps/api/core";
export async function save(n: string) {
  await invoke("save_note", { n });
  await invoke("plugin:fs|read_file", {});
}
"#,
    );
    assert_eq!(
        call_names(&f),
        [(EndpointKind::TauriCommand, None, "save_note".into(), false)]
    );
    // Without the Tauri import a function called invoke is something else.
    let g = facts("src/x.ts", "invoke('save_note');\n");
    assert!(g.calls.is_empty());
}

#[test]
fn ts_react_controls() {
    let f = facts(
        "src/App.tsx",
        r#"
export function App() {
  return (
    <main>
      <button onClick={save}>Save note</button>
      <button onClick={() => remove(1)}>Delete</button>
      <button>Do nothing</button>
      <form onSubmit={handleSubmit(onSave)}><button>Send</button></form>
      <form action="/api/upload" method="post"><input /></form>
      <Link to="/notes">All notes</Link>
      <a href="https://example.com">Out</a>
    </main>
  );
}
"#,
    );
    let ui: Vec<(UiKind, Option<String>, Option<String>)> = f
        .ui
        .iter()
        .map(|u| (u.kind, u.label.clone(), u.handler.clone()))
        .collect();
    assert_eq!(
        ui,
        [
            (UiKind::Button, Some("Save note".into()), Some("save".into())),
            (UiKind::Button, Some("Delete".into()), Some("remove".into())),
            (UiKind::Button, Some("Do nothing".into()), None),
            (UiKind::Form, Some("Send".into()), Some("onSave".into())),
            (UiKind::Button, Some("Send".into()), Some("(form submit)".into())),
            (UiKind::Form, None, Some("/api/upload".into())),
            (UiKind::Link, Some("All notes".into()), Some("/notes".into())),
        ]
    );
    assert!(f.ui.iter().all(|u| u.component.as_deref() == Some("App")));
    assert_eq!(
        call_names(&f),
        [(EndpointKind::Http, Some("POST".into()), "/api/upload".into(), false)]
    );
}

#[test]
fn ts_jsx_files_share_the_extractors() {
    let f = facts(
        "src/App.jsx",
        "export const App = () => <button onClick={save}>Save</button>;\n",
    );
    assert_eq!(f.ui.len(), 1);
    assert_eq!(f.ui[0].handler.as_deref(), Some("save"));
    assert_eq!(f.ui[0].component.as_deref(), Some("App"));
    let d = f.defs.iter().find(|d| d.name == "App").unwrap();
    assert_eq!(d.kind, DefKind::Component);
}

#[test]
fn ts_definitions_refs_imports() {
    let f = facts(
        "src/svc.ts",
        r#"
import Default, { a, b as c } from "./lib";
import * as ns from "ns";
export function run() { helper(1); new Thing(); }
function helper(x: number) {}
class Store {
  save(n: string) { throw new Error("Not implemented yet"); }
  load = () => this.cache.get("x");
}
export const LIMIT = 10;
"#,
    );
    let defs: Vec<(&str, DefKind, bool)> = f
        .defs
        .iter()
        .map(|d| (d.name.as_str(), d.kind, d.is_stub))
        .collect();
    assert_eq!(
        defs,
        [
            ("run", DefKind::Function, false),
            ("helper", DefKind::Function, true),
            ("Store", DefKind::Class, false),
            ("Store.save", DefKind::Method, true),
            ("Store.load", DefKind::Method, false),
            ("LIMIT", DefKind::Constant, false),
        ]
    );
    let refs: Vec<(Option<&str>, &str)> = f
        .refs
        .iter()
        .map(|r| (r.caller.as_deref(), r.callee.as_str()))
        .collect();
    assert!(refs.contains(&(Some("run"), "helper")));
    assert!(refs.contains(&(Some("run"), "Thing")));
    assert!(refs.contains(&(Some("Store.load"), "get")));
    assert_eq!(f.imports.len(), 2);
    assert_eq!(f.imports[0].module, "./lib");
    assert_eq!(f.imports[0].names, ["Default", "a", "b"]);
    assert_eq!(f.imports[1].names, ["ns"]);
}
