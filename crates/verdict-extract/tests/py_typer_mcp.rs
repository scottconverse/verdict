//! Typer commands and Python MCP tools.

mod py_common;

use py_common::*;
use verdict_extract::{EndpointKind, Framework};

fn cmds(f: &verdict_extract::Facts) -> Vec<String> {
    let mut v: Vec<String> = eps(f, EndpointKind::CliCommand)
        .into_iter()
        .map(|(_, name, handler, router)| {
            format!(
                "{name} -> {handler}{}",
                if router.is_some() { " (unmounted)" } else { "" }
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn typer_command_names() {
    let f = one(r#"import typer

app = typer.Typer()

@app.command()
def scan_tree(path: str):
    pass

@app.command("export")
def do_export():
    pass

@app.command(name="Check")
def check():
    pass

@app.command
def Bare_One():
    pass

@app.callback()
def main():
    pass
"#);
    assert_eq!(
        cmds(&f),
        [
            "Check -> check",
            "bare-one -> Bare_One",
            "export -> do_export",
            "scan-tree -> scan_tree",
        ]
    );
    assert!(f.frameworks().contains(&Framework::Typer));
    let e = f.endpoints().find(|e| e.key.name == "scan-tree").unwrap();
    assert_eq!(e.loc.line, 5);
}

#[test]
fn typer_groups_across_files() {
    let f = project(&[
        (
            "cli/main.py",
            "import typer\nfrom cli import users, db\nfrom cli.reports import reports_app\n\napp = typer.Typer()\napp.add_typer(users.app, name=\"users\")\napp.add_typer(db.app)\napp.add_typer(reports_app, name=\"rep\")\n\n@app.command()\ndef version():\n    pass\n",
        ),
        ("cli/__init__.py", ""),
        (
            "cli/users.py",
            "import typer\napp = typer.Typer()\n@app.command()\ndef add():\n    pass\n@app.command('rm')\ndef remove():\n    pass\n",
        ),
        (
            "cli/db.py",
            "import typer\napp = typer.Typer(name=\"database\")\n@app.command()\ndef migrate():\n    pass\n",
        ),
        (
            "cli/reports.py",
            "import typer\nreports_app = typer.Typer(name='ignored')\nsub = typer.Typer()\nreports_app.add_typer(sub, name='deep')\n@reports_app.command()\ndef run():\n    pass\n@sub.command()\ndef go():\n    pass\n",
        ),
    ]);
    assert_eq!(
        cmds(&f),
        [
            "database migrate -> migrate",
            "rep deep go -> go",
            "rep run -> run",
            "users add -> add",
            "users rm -> remove",
            "version -> version",
        ]
    );
}

#[test]
fn click_groups_are_not_typer() {
    let f = project(&[
        (
            "cli/main.py",
            "import click\n\n@click.group()\ndef cli():\n    pass\n\n@cli.command()\ndef hello():\n    pass\n",
        ),
        (
            "cli/other.py",
            "from cli.main import cli\n\n@cli.command()\ndef more():\n    pass\n",
        ),
    ]);
    assert!(cmds(&f).is_empty(), "{:?}", cmds(&f));
}

fn tools(f: &verdict_extract::Facts) -> Vec<String> {
    let mut v: Vec<String> = eps(f, EndpointKind::McpTool)
        .into_iter()
        .map(|(_, name, handler, router)| {
            format!(
                "{name} -> {handler}{}",
                if router.is_some() {
                    " (unresolved)"
                } else {
                    ""
                }
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn fastmcp_tools() {
    let f = one(r#"from mcp.server.fastmcp import FastMCP

mcp = FastMCP("demo")

@mcp.tool()
def add_note(text: str) -> str:
    return text

@mcp.tool(name="remove_note")
async def delete_it(id: int):
    ...

@mcp.tool
def bare():
    pass

@mcp.tool("positional")
def p():
    pass

def late(x):
    pass

mcp.add_tool(late, name="added")
mcp.add_tool(late)
"#);
    assert_eq!(
        tools(&f),
        [
            "add_note -> add_note",
            "added -> late",
            "bare -> bare",
            "late -> late",
            "positional -> p",
            "remove_note -> delete_it",
        ]
    );
    assert!(f.frameworks().contains(&Framework::McpPython));
    assert_eq!(
        f.endpoints()
            .find(|e| e.key.name == "add_note")
            .unwrap()
            .loc
            .line,
        5
    );
}

#[test]
fn server_tools_and_tool_listings_across_files() {
    let f = project(&[
        (
            "srv/server.py",
            "from mcp.server import Server\nimport mcp.types as types\n\nserver = Server(\"x\")\n\n@server.list_tools()\nasync def list_tools():\n    return [types.Tool(name=\"search\", description=\"d\", inputSchema={}), types.Tool(name=\"fetch\")]\n",
        ),
        (
            "srv/tools.py",
            "from srv.server import server\n\n@server.tool()\ndef extra():\n    pass\n",
        ),
        (
            "srv/agent.py",
            "from pydantic_ai import Agent\nfrom srv.brain import agent\n\n@agent.tool\ndef not_mcp():\n    pass\n",
        ),
        ("srv/brain.py", "agent = make_agent()\n"),
    ]);
    assert_eq!(tools(&f), ["extra -> extra", "fetch -> ", "search -> "]);
    let extra = f.endpoints().find(|e| e.key.name == "extra").unwrap();
    assert_eq!(extra.router, None, "the import resolves to the Server");
}

#[test]
fn local_agent_tool_is_not_mcp() {
    let f = one(r#"from pydantic_ai import Agent

agent = Agent("x")

@agent.tool
def ask():
    pass
"#);
    assert!(tools(&f).is_empty());
}

#[test]
fn tool_from_an_unseen_server_stays_unresolved() {
    let f = one(r#"from vendor_pkg import mcp

@mcp.tool()
def go():
    pass
"#);
    assert_eq!(tools(&f), ["go -> go (unresolved)"]);
}

#[test]
fn a_group_nothing_mounts_is_not_reachable_when_there_is_a_real_entry_point() {
    let f = project(&[
        (
            "cli/main.py",
            "import typer\napp = typer.Typer()\n@app.command()\ndef version():\n    pass\nif __name__ == '__main__':\n    app()\n",
        ),
        (
            "cli/extra.py",
            "import typer\napp = typer.Typer()\n@app.command()\ndef lonely():\n    pass\n",
        ),
    ]);
    assert_eq!(
        cmds(&f),
        ["lonely -> lonely (unmounted)", "version -> version"]
    );
}

#[test]
fn two_unmarked_roots_are_both_unknown() {
    let f = project(&[
        (
            "cli/a.py",
            "import typer\napp = typer.Typer()\n@app.command()\ndef one():\n    pass\n",
        ),
        (
            "cli/b.py",
            "import typer\napp = typer.Typer()\n@app.command()\ndef two():\n    pass\n",
        ),
    ]);
    assert_eq!(
        cmds(&f),
        ["one -> one (unmounted)", "two -> two (unmounted)"]
    );
}

#[test]
fn the_entry_point_can_be_run_from_another_file() {
    let f = project(&[
        ("cli/__init__.py", ""),
        (
            "cli/app.py",
            "import typer\napp = typer.Typer()\nother = typer.Typer()\n@app.command()\ndef one():\n    pass\n@other.command()\ndef two():\n    pass\n",
        ),
        (
            "cli/__main__.py",
            "from cli.app import app\n\nif __name__ == '__main__':\n    app()\n",
        ),
    ]);
    assert_eq!(cmds(&f), ["one -> one", "two -> two (unmounted)"]);
}
