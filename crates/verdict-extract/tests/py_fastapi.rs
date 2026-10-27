//! FastAPI facts: routes, router prefixes, include_router chains across files.

mod py_common;

use py_common::*;
use verdict_extract::{EndpointKind, Framework};

#[test]
fn app_and_router_routes_with_handlers_and_lines() {
    let f = one(r#"from fastapi import FastAPI, APIRouter

app = FastAPI()
router = APIRouter(prefix="/items")


@app.get("/health")
def health():
    return "ok"


@router.post("/{item_id}/tags", status_code=201)
async def add_tag(item_id: int):
    ...


@app.api_route("/multi", methods=["GET", "post"])
def multi():
    pass


@app.websocket("/ws")
async def socket(ws):
    pass


@app.put(path="/keyword")
def keyword():
    pass

app.include_router(router)
"#);
    assert_eq!(
        routes(&f),
        [
            "* /ws",
            "GET /health",
            "GET /multi",
            "POST /items/{}/tags",
            "POST /multi",
            "PUT /keyword",
        ]
    );
    let e = endpoint(&f, "/health");
    assert_eq!(e.handler.as_deref(), Some("health"));
    assert_eq!(e.loc.file, "app/main.py");
    assert_eq!(e.loc.line, 7);
    assert_eq!(e.router, None, "mounted on an app: fully resolved");
    assert_eq!(endpoint(&f, "/items/{}/tags").loc.line, 12);
    assert_eq!(
        endpoint(&f, "/items/{}/tags").handler.as_deref(),
        Some("add_tag")
    );
    assert!(f.frameworks().contains(&Framework::FastApi));
}

#[test]
fn cross_file_prefix_chain() {
    let f = project(&[
        (
            "app/main.py",
            "from fastapi import FastAPI\nfrom app.api import api_router\nfrom app.routers import users\n\napp = FastAPI()\napp.include_router(api_router, prefix=\"/api/v1\")\napp.include_router(users.router, prefix=\"/legacy\")\n",
        ),
        (
            "app/api.py",
            "from fastapi import APIRouter\nfrom .routers import notes\n\napi_router = APIRouter()\napi_router.include_router(notes.router, prefix=\"/notes\", tags=[\"n\"])\n",
        ),
        ("app/routers/__init__.py", ""),
        (
            "app/routers/notes.py",
            "from fastapi import APIRouter\n\nrouter = APIRouter(prefix=\"/inner\")\n\n\n@router.get(\"/{note_id}\")\ndef read_note(note_id: int):\n    pass\n\n\n@router.post(\"\")\ndef create_note():\n    pass\n",
        ),
        (
            "app/routers/users.py",
            "from fastapi import APIRouter\n\nrouter = APIRouter(prefix=\"/users\")\n\n\n@router.get(\"/me\")\ndef me():\n    pass\n",
        ),
    ]);
    assert_eq!(
        routes(&f),
        [
            "GET /api/v1/notes/inner/{}",
            "GET /legacy/users/me",
            "POST /api/v1/notes/inner",
        ]
    );
    for e in f.endpoints() {
        assert_eq!(e.router, None, "{} is mounted on the app", e.key.name);
    }
    let read = f
        .endpoints()
        .find(|e| e.handler.as_deref() == Some("read_note"))
        .unwrap();
    assert_eq!(read.loc.file, "app/routers/notes.py");
    assert_eq!(read.loc.line, 6);
    // The mount records are resolved to files too.
    let main = f.file("app/main.py").unwrap();
    assert!(
        main.mounts
            .iter()
            .any(|m| m.child == "app/api.py:api_router" && m.prefix == "/api/v1")
    );
}

#[test]
fn imports_by_every_spelling_resolve() {
    let f = project(&[
        (
            "src/app/main.py",
            "import app.routers.a as ra\nimport app.routers.b\nfrom app.routers.c import router as rc\nfrom . import d\nfrom fastapi import FastAPI\n\napp = FastAPI()\napp.include_router(ra.router, prefix=\"/a\")\napp.include_router(app.routers.b.router, prefix=\"/b\")\napp.include_router(rc, prefix=\"/c\")\napp.include_router(d.router, prefix=\"/d\")\n",
        ),
        (
            "src/app/routers/a.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get(\"/x\")\ndef x(): pass\n",
        ),
        (
            "src/app/routers/b.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get(\"/x\")\ndef x(): pass\n",
        ),
        (
            "src/app/routers/c.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get(\"/x\")\ndef x(): pass\n",
        ),
        (
            "src/app/d.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get(\"/x\")\ndef x(): pass\n",
        ),
    ]);
    assert_eq!(routes(&f), ["GET /a/x", "GET /b/x", "GET /c/x", "GET /d/x"]);
}

#[test]
fn re_exported_routers_are_followed() {
    let f = project(&[
        (
            "app/main.py",
            "from fastapi import FastAPI\nfrom app.api import router\n\napp = FastAPI()\napp.include_router(router, prefix=\"/v1\")\n",
        ),
        ("app/api/__init__.py", "from .items import router\n"),
        (
            "app/api/items.py",
            "from fastapi import APIRouter\nrouter = APIRouter(prefix=\"/items\")\n@router.get(\"/\")\ndef items(): pass\n",
        ),
    ]);
    assert_eq!(routes(&f), ["GET /v1/items"]);
}

#[test]
fn unmounted_router_keeps_its_own_path_and_router() {
    let f = project(&[
        (
            "app/main.py",
            "from fastapi import FastAPI\napp = FastAPI()\n@app.get('/up')\ndef up(): pass\n",
        ),
        (
            "app/orphan.py",
            "from fastapi import APIRouter\nrouter = APIRouter(prefix='/orphan')\n@router.get('/a')\ndef a(): pass\n",
        ),
    ]);
    let orphan = endpoint(&f, "/orphan/a");
    assert_eq!(orphan.router.as_deref(), Some("app/orphan.py:router"));
    assert_eq!(endpoint(&f, "/up").router, None);
}

#[test]
fn unreadable_prefix_leaves_the_router_set() {
    let f = project(&[
        (
            "app/main.py",
            "from fastapi import FastAPI\nfrom app.r import router\nfrom app.core import settings\n\napp = FastAPI()\napp.include_router(router, prefix=settings.API_PREFIX)\n",
        ),
        (
            "app/r.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get('/a')\ndef a(): pass\n",
        ),
    ]);
    let e = endpoint(&f, "/a");
    assert_eq!(e.router.as_deref(), Some("app/r.py:router"));
    let m = &f.file("app/main.py").unwrap().mounts;
    assert!(m.iter().any(|m| m.prefix == "?settings.API_PREFIX"));
}

#[test]
fn same_file_constants_and_f_strings_in_prefixes() {
    let f = one(r#"from fastapi import FastAPI, APIRouter
BASE = "/api"
V = BASE + "/v2"
app = FastAPI()
r = APIRouter(prefix=f"{V}/things")

@r.get(f"/{BASE}x/{{id}}")
def t(): pass

app.include_router(r, prefix=BASE)
"#);
    // r's own prefix "/api/v2/things" then the include prefix "/api".
    assert_eq!(routes(&f).len(), 1);
    assert!(
        routes(&f)[0].starts_with("GET /api/api/v2/things/"),
        "{:?}",
        routes(&f)
    );
}

#[test]
fn computed_route_path_is_a_wildcard_segment_not_a_guess() {
    let f = one(r#"from fastapi import FastAPI
app = FastAPI()
@app.get(make_path())
def t(): pass
"#);
    assert_eq!(routes(&f), ["GET /{}"]);
}

#[test]
fn router_mounted_twice_gives_both_paths() {
    let f = project(&[
        (
            "app/main.py",
            "from fastapi import FastAPI\nfrom app.r import router\napp = FastAPI()\napp.include_router(router, prefix='/v1')\napp.include_router(router, prefix='/v2')\n",
        ),
        (
            "app/r.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get('/a')\ndef a(): pass\n",
        ),
    ]);
    assert_eq!(routes(&f), ["GET /v1/a", "GET /v2/a"]);
}

#[test]
fn mounted_sub_app_and_add_api_route() {
    let f = one(r#"from fastapi import FastAPI
app = FastAPI()
admin = FastAPI()
app.mount("/admin", admin)
app.mount("/static", StaticFiles(directory="s"))

@admin.get("/stats")
def stats(): pass

def list_things(): pass
app.add_api_route("/things", list_things, methods=["GET", "HEAD"])
"#);
    assert_eq!(
        routes(&f),
        ["GET /admin/stats", "GET /things", "HEAD /things"]
    );
    assert_eq!(
        f.endpoints()
            .find(|e| e.key.name == "/things")
            .unwrap()
            .handler
            .as_deref(),
        Some("list_things")
    );
}

#[test]
fn other_frameworks_decorators_are_not_routes() {
    let f = one(r#"from flask import Flask, Blueprint
app = Flask(__name__)
bp = Blueprint("x", __name__)

@app.get("/flask")
def a(): pass

@bp.get("/bp")
def b(): pass

@pytest.mark.get("/nope")
def c(): pass
"#);
    assert!(routes(&f).is_empty(), "{:?}", routes(&f));
    assert!(!f.frameworks().contains(&Framework::FastApi));
}

#[test]
fn router_passed_in_stays_unresolved() {
    let f = one(r#"from fastapi import APIRouter

def register(router: APIRouter):
    @router.get("/late")
    def late(): pass
"#);
    let e = endpoint(&f, "/late");
    assert_eq!(e.router.as_deref(), Some("app/main.py:router"));
    assert_eq!(e.handler.as_deref(), Some("late"));
}

#[test]
fn methods_as_handlers_use_the_definition_name() {
    let f = one(r#"from fastapi import APIRouter
router = APIRouter()
class V:
    @router.get("/m")
    def m(self): pass
"#);
    assert_eq!(endpoint(&f, "/m").handler.as_deref(), Some("V.m"));
}

#[test]
fn no_http_endpoints_without_fastapi() {
    let f = one("def f():\n    pass\n");
    assert!(f.endpoints().all(|e| e.key.kind != EndpointKind::Http));
    assert!(f.frameworks().is_empty());
}

#[test]
fn extraction_is_deterministic() {
    let files = [
        (
            "a/main.py",
            "from fastapi import FastAPI\nfrom a.r import router\napp = FastAPI()\napp.include_router(router, prefix='/x')\napp.include_router(router, prefix='/y')\n",
        ),
        (
            "a/r.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get('/p')\ndef p(): pass\n",
        ),
    ];
    assert_eq!(project(&files), project(&files));
}
