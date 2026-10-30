//! `vdt setup`: the install wizard the per-OS packages launch.
//!
//! It is a small localhost page, so the installer needs no terminal and no system GUI toolkit:
//! double-click the package, the default browser opens this page, choose the project and the hosts,
//! press Install. It copies the core, registers the chosen hosts with the exact files the adapters
//! already write, makes a launcher for the chosen project, starts the page and closes.
//!
//! It performs only what the human pressed: the host registrations are recorded as human
//! decisions by the adapters, exactly like `vdt mcp-install --i-am-the-human`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Form, Router,
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tokio::sync::oneshot;
use verdict_adapters::{McpHost, McpRequest};

use crate::{bind, esc, new_token};

/// What `vdt setup` was asked for.
#[derive(Debug, Clone)]
pub struct SetupOptions {
    /// The project to install for (the wizard can change it).
    pub project: Option<PathBuf>,
    /// Port; 0 picks a free one.
    pub port: u16,
    /// Open the browser. Tests and headless runs turn this off.
    pub open: bool,
    /// Where to put the core; the platform default when absent.
    pub install_dir: Option<PathBuf>,
    /// Headless: install this project without serving the page (no host decisions).
    pub silent: bool,
    /// Hosts to register in silent mode.
    pub hosts: Vec<String>,
}

impl Default for SetupOptions {
    fn default() -> Self {
        SetupOptions {
            project: None,
            port: 0,
            open: true,
            install_dir: None,
            silent: false,
            hosts: Vec::new(),
        }
    }
}

struct Setup {
    token: String,
    project: Option<PathBuf>,
    install_dir: Option<PathBuf>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

type St = Arc<Setup>;

/// Platform default install folder for the core.
pub fn default_install_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Verdict").join("bin")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".local").join("bin")
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "vdt.exe" } else { "vdt" }
}

/// The core binary that sits beside this setup binary.
fn core_beside_setup() -> Result<PathBuf, String> {
    let me = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    if me.file_name().and_then(|n| n.to_str()) == Some(exe_name()) {
        return Ok(me);
    }
    let beside = me
        .parent()
        .ok_or("this program has no parent folder")?
        .join(exe_name());
    if beside.is_file() {
        // The download must match the checksum published beside the core; a missing or different
        // checksum refuses the install instead of copying an unverified program.
        let sum_file = beside.with_file_name(format!("{}.sha256", exe_name()));
        let want = std::fs::read_to_string(&sum_file).map_err(|e| {
            format!(
                "the checksum {} cannot be read: {e}; refusing to install an unverified download",
                sum_file.display()
            )
        })?;
        let want = want
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let bytes =
            std::fs::read(&beside).map_err(|e| format!("cannot read {}: {e}", beside.display()))?;
        let got = verdict_core::sha256::hex(&bytes);
        if want != got {
            return Err(format!(
                "checksum mismatch for {}: the download is damaged or was changed",
                beside.display()
            ));
        }
        Ok(beside)
    } else {
        Err(format!(
            "cannot find the core {} beside the setup program",
            beside.display()
        ))
    }
}

/// Does the machine look like it has this host?
fn detected(host: &str) -> bool {
    let on_path = |name: &str| {
        std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths).any(|p| {
                    p.join(name).is_file()
                        || p.join(format!("{name}.exe")).is_file()
                        || p.join(format!("{name}.cmd")).is_file()
                })
            })
            .unwrap_or(false)
    };
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    match host {
        "claude" => on_path("claude") || home.as_ref().is_some_and(|h| h.join(".claude").is_dir()),
        "codex" => on_path("codex") || home.as_ref().is_some_and(|h| h.join(".codex").is_dir()),
        "cursor" => on_path("cursor") || home.as_ref().is_some_and(|h| h.join(".cursor").is_dir()),
        "dsh" => {
            std::env::var_os("DSH_HOME").is_some()
                || home.as_ref().is_some_and(|h| h.join(".dsh").is_dir())
        }
        "antigravity" => {
            on_path("antigravity")
                || home
                    .as_ref()
                    .is_some_and(|h| h.join(".antigravity").is_dir())
        }
        _ => false,
    }
}

fn page(token: &str, project: &Path, install_dir: &Path) -> String {
    let mut hosts = String::new();
    for host in ["claude", "cursor", "codex", "dsh", "antigravity"] {
        let found = detected(host);
        hosts.push_str(&format!(
            "<label class=\"host\"><input type=\"checkbox\" name=\"host\" value=\"{host}\"{}> \
             <strong>{host}</strong> <span class=\"muted\">{}</span></label>",
            if found { " checked" } else { "" },
            if found {
                "detected on this machine"
            } else {
                "not detected; choose it only if you use it"
            }
        ));
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Install Verdict</title>\
         <style>{STYLE}</style></head><body><main>\
         <h1>Install Verdict</h1>\
         <p class=\"lead\">Verdict installs once and then lives on your machine. Nothing here is \
         installed until you press the button.</p>\
         <form method=\"post\" action=\"/install\">\
         <input type=\"hidden\" name=\"token\" value=\"{token}\">\
         <label for=\"project\">Project folder</label>\
         <input id=\"project\" name=\"project\" size=\"60\" value=\"{project}\" required>\
         <label for=\"dir\">Install folder</label>\
         <input id=\"dir\" name=\"install_dir\" size=\"60\" value=\"{dir}\">\
         <h2>Register the MCP server with</h2>\
         {hosts}\
         <input type=\"hidden\" name=\"hosts\" id=\"hosts\">\
         <p class=\"muted\">Registering writes that host's own configuration file and records your \
         decision in the project's ledger. You can register another host later from the page.</p>\
         <button type=\"submit\">Install Verdict</button>\
         </form></main>\
         <script>\
         document.querySelector('form').addEventListener('submit', function(){{\
           var v = Array.from(document.querySelectorAll('input[name=host]:checked'))\
             .map(function(e){{return e.value;}}).join(',');\
           document.getElementById('hosts').value = v;\
         }});\
         </script></body></html>",
        project = esc(&project.to_string_lossy()),
        dir = esc(&install_dir.to_string_lossy()),
    )
}

const STYLE: &str = "
body{font:20px/1.5 system-ui,sans-serif;color:#111;background:#fff;margin:0}
main{max-width:760px;margin:40px auto;padding:0 20px 60px}
h1{font-size:34px;margin:0 0 10px}h2{font-size:24px;margin:28px 0 8px}
label{display:block;margin:18px 0 6px;font-weight:600}
input[type=text],input:not([type]){width:100%;padding:10px;font:inherit;border:2px solid #111;border-radius:4px}
.host{font-weight:400;margin:10px 0}.muted{color:#555;font-size:16px}
button{margin-top:28px;padding:14px 28px;font:inherit;font-weight:700;color:#fff;background:#111;border:0;border-radius:6px;cursor:pointer}
.lead{font-size:22px}
";

async fn index(State(st): State<St>) -> Html<String> {
    let project = st
        .project
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = st.install_dir.clone().unwrap_or_else(default_install_dir);
    Html(page(&st.token, &project, &dir))
}

#[derive(Deserialize)]
struct InstallForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    install_dir: String,
    #[serde(default)]
    hosts: String,
}

async fn install(State(st): State<St>, Form(f): Form<InstallForm>) -> Response {
    if f.token != st.token {
        return Html(
            "<p>Refused: the page token is missing or wrong. Reload the page and try again.</p>"
                .to_string(),
        )
        .into_response();
    }
    let project = PathBuf::from(f.project.trim());
    if !project.is_dir() {
        return Html(format!(
            "<p>{} is not a folder. Go back and choose the project folder.</p>",
            esc(&project.to_string_lossy())
        ))
        .into_response();
    }
    let dir = if f.install_dir.trim().is_empty() {
        st.install_dir.clone().unwrap_or_else(default_install_dir)
    } else {
        PathBuf::from(f.install_dir.trim())
    };
    let hosts: Vec<String> = f
        .hosts
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect();
    let project_label = project.clone();
    let result =
        tokio::task::spawn_blocking(move || install_core(&project, &dir, &hosts, true)).await;
    match result {
        Ok(Ok(installed)) => {
            if let Some(tx) = st.shutdown.lock().ok().and_then(|mut s| s.take()) {
                let _ = tx.send(());
            }
            Html(success_page(&installed.vdt, &project_label)).into_response()
        }
        Ok(Err(e)) => Html(format!(
            "<p>The install did not finish: {}</p><p>What was completed is named above; nothing \
             was undone.</p><p><a href=\"/\">Back</a></p>",
            esc(&e)
        ))
        .into_response(),
        Err(e) => Html(format!(
            "<p>The install stopped: {}</p><p><a href=\"/\">Back</a></p>",
            esc(&e.to_string())
        ))
        .into_response(),
    }
}

fn success_page(vdt: &Path, project: &Path) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Verdict installed</title>\
         <style>{STYLE}</style><main><h1>Verdict is installed</h1>\
         <p>The page for <strong>{project}</strong> is opening in your browser. From now on the \
         Verdict shortcut starts it for that project.</p>\
         <p class=\"muted\">Installed at {vdt}</p></main>",
        project = esc(&project.to_string_lossy()),
        vdt = esc(&vdt.to_string_lossy())
    )
}

struct Installed {
    vdt: PathBuf,
}

fn install_core(
    project: &Path,
    dir: &Path,
    hosts: &[String],
    open_page: bool,
) -> Result<Installed, String> {
    let core = core_beside_setup()?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let target = dir.join(exe_name());
    std::fs::copy(&core, &target).map_err(|e| {
        format!(
            "cannot copy {} to {}: {e}",
            core.display(),
            target.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    // The hosts the human chose. `mcp_command` writes the host's own file and records the human
    // decision; it is the same path `vdt mcp-install` uses.
    for host in hosts {
        let Some(host) = McpHost::parse(host) else {
            return Err(format!("{host:?} is not a supported host"));
        };
        let req = McpRequest {
            host,
            project: Some(project.to_path_buf()),
            vdt_path: Some(target.to_string_lossy().to_string()),
            codex_home: None,
            i_am_the_human: true,
        };
        let mut out = Vec::new();
        verdict_adapters::mcp_command(&req, project, &mut out).map_err(|e| {
            format!(
                "the core at {} was installed, but {} was not registered: {e}",
                target.display(),
                host.title()
            )
        })?;
    }
    if open_page {
        launcher(project, &target).map_err(|e| {
            format!(
                "the core at {} was installed, but the launcher was not created: {e}",
                target.display()
            )
        })?;
        // The wizard is already listening on its own port; the installed server needs a free one.
        let server_port = free_port()?;
        if let Err(e) = open_project_page(project, &target, server_port) {
            return Err(format!(
                "installed the core at {} and the launcher, but not the page: {e}",
                target.display()
            ));
        }
    }
    // Best effort: re-opening the installed app opens the project chosen here. If this cannot be
    // written, the app falls back to the wizard; the install itself is still complete.
    if let Err(e) = record_last_project_in(&state_dir(), project) {
        eprintln!("warning: the chosen project was not remembered for re-opening: {e}");
    }
    Ok(Installed { vdt: target })
}

/// The folder holding Verdict's small per-user state. `VERDICT_STATE_DIR` overrides it (tests).
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("VERDICT_STATE_DIR") {
        return PathBuf::from(dir);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".verdict"))
        .unwrap_or_else(|| PathBuf::from(".verdict"))
}

/// Remember `project` as the one the installed app should open. Written atomically: a temp file is
/// renamed over the old one, so a crash cannot leave half a path behind.
fn record_last_project_in(state: &Path, project: &Path) -> Result<(), String> {
    std::fs::create_dir_all(state).map_err(|e| format!("cannot make {}: {e}", state.display()))?;
    let project = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let file = state.join("last-project");
    let tmp = state.join(format!("last-project.tmp{}", std::process::id()));
    std::fs::write(&tmp, format!("{}\n", project.display()))
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("cannot replace {}: {e}", file.display()))
}

/// The project the installed app should open, when one was recorded and it still exists. The
/// macOS bundle's launcher reads the same file; this reader is used by the tests that pin the
/// format, and by any future in-process use.
#[allow(dead_code)]
fn last_project_in(state: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(state.join("last-project")).ok()?;
    let project = PathBuf::from(text.trim());
    project.is_dir().then_some(project)
}

/// A desktop entry, shortcut or copied app bundle that starts the page for this project.
fn launcher(project: &Path, vdt: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let desktop = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .map(|h| h.join("Desktop"))
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&desktop).ok();
        let lnk = desktop.join("Verdict.lnk");
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');\
             $s.TargetPath='{}';\
             $s.Arguments='serve --open --project \"{}\"';$s.WorkingDirectory='{}';$s.Save()",
            lnk.to_string_lossy().replace('\'', "''"),
            vdt.to_string_lossy().replace('\'', "''"),
            project.to_string_lossy().replace('\'', "''"),
            project.to_string_lossy().replace('\'', "''"),
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("cannot create the shortcut: {e}"))?;
        if !status.success() {
            return Err("the shortcut could not be created".into());
        }
    }
    #[cfg(target_os = "macos")]
    {
        // The app bundle itself is the launcher; `project` and `vdt` are used by the page opener
        // that runs after this, not here.
        let _ = (project, vdt);
        // Reuse the app bundle this setup binary lives in, when there is one.
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        if let Some(app) = me
            .ancestors()
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
            .map(Path::to_path_buf)
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let dest = home.join("Applications").join("Verdict.app");
            let _ = std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::remove_dir_all(&dest);
            copy_dir(&app, &dest)?;
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = home.join(".local").join("share").join("applications");
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Verdict\nComment=Open the Verdict page for {}\n\
             Exec=\"{}\" serve --open --project \"{}\"\nPath={}\nTerminal=false\nCategories=Development;\n",
            project.display(),
            vdt.display(),
            project.display(),
            project.display()
        );
        std::fs::write(dir.join("verdict.desktop"), entry)
            .map_err(|e| format!("cannot write the desktop entry: {e}"))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let to = dst.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// A free loopback port, discovered by binding port 0 and dropping the listener. The wizard keeps
/// its own port; the server it starts gets this one.
fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot choose a port for the page: {e}"))?;
    let port = l
        .local_addr()
        .map_err(|e| format!("cannot read the chosen port: {e}"))?
        .port();
    drop(l);
    Ok(port)
}

/// The argv used to start the page for `project` on `port`. Factored out so a test can assert the
/// port is actually passed: the installer verifies and opens the same server it started.
fn serve_args(project: &Path, port: u16) -> Vec<String> {
    vec![
        "serve".into(),
        "--project".into(),
        project.to_string_lossy().to_string(),
        "--port".into(),
        port.to_string(),
    ]
}

fn open_project_page(project: &Path, vdt: &Path, port: u16) -> Result<(), String> {
    let args = serve_args(project, port);
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new(vdt)
            .args(&args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    #[cfg(not(windows))]
    let spawned = std::process::Command::new(vdt)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = spawned.map_err(|e| format!("the page did not start: {e}"))?;
    // Only call the page open when the server answering on the port is the instance this installer
    // started, says it is Verdict, and serves the project chosen here. A bare HTTP 200 from
    // anything else is not success.
    // The page prints its one-time link on its own output, which only this installer reads. The
    // link lets the browser it opens in; nothing else on the computer sees the code.
    let out = child
        .stdout
        .take()
        .ok_or("the page did not give its link")?;
    let link = read_link(std::io::BufReader::new(out), port)?;
    wait_until_ready(project, port, child.id(), Duration::from_secs(15))?;
    open_url(&link);
    Ok(())
}

/// Read the one-time link `vdt serve` prints for `port`.
fn read_link(mut out: impl std::io::BufRead, port: u16) -> Result<String, String> {
    let want = page_url(port);
    let mut line = String::new();
    for _ in 0..20 {
        line.clear();
        match out.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if let Some(at) = line.find(&want) {
                    let link = line[at..].trim().to_string();
                    if link.contains("?k=") {
                        return Ok(link);
                    }
                }
            }
        }
    }
    Err("the page started but did not print its one-time link".into())
}

/// The URL the installer opens for the server it started on `port`.
fn page_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// Poll `/readyz` until the page identifies itself as Verdict for `project` and is the process this
/// installer started (`pid`), or the deadline passes.
fn wait_until_ready(project: &Path, port: u16, pid: u32, deadline: Duration) -> Result<(), String> {
    let want = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let started = std::time::Instant::now();
    let mut last = "no answer".to_string();
    while started.elapsed() < deadline {
        match read_readyz(port) {
            Ok(v) => {
                let app = v.get("app").and_then(|x| x.as_str()).unwrap_or("");
                let served = v.get("project").and_then(|x| x.as_str()).unwrap_or("");
                let answered_pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0);
                if app == "verdict" && answered_pid == u64::from(pid) && Path::new(served) == want {
                    return Ok(());
                }
                last = format!("it answered as {app:?} for {served:?} (pid {answered_pid})");
            }
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "the page did not come up for {} within {} seconds ({last})",
        want.display(),
        deadline.as_secs()
    ))
}

/// One `GET /readyz` over the loopback.
fn read_readyz(port: u16) -> Result<serde_json::Value, String> {
    use std::io::{Read as _, Write as _};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("cannot connect to the page: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request =
        format!("GET /readyz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("the page refused the readiness request: {e}"))?;
    let mut buf = String::new();
    stream
        .read_to_string(&mut buf)
        .map_err(|e| format!("the readiness answer could not be read: {e}"))?;
    let body = buf.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body).map_err(|e| format!("the answer is not readiness JSON: {e}"))
}

/// Open a URL in the user's browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// Run the wizard (or a silent install) and return after it finishes.
pub fn run_setup(opts: SetupOptions) -> std::io::Result<()> {
    if opts.silent {
        let project = opts
            .project
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let dir = opts.install_dir.clone().unwrap_or_else(default_install_dir);
        install_core(&project, &dir, &opts.hosts, !opts.silent).map_err(std::io::Error::other)?;
        println!(
            "Installed {} for {}",
            dir.join(exe_name()).display(),
            project.display()
        );
        return Ok(());
    }
    let listener = bind(opts.port)?;
    let addr = listener.local_addr()?;
    let token = new_token();
    let (tx, rx) = oneshot::channel();
    let state: St = Arc::new(Setup {
        token: token.clone(),
        project: opts.project.clone(),
        install_dir: opts.install_dir.clone(),
        shutdown: Mutex::new(Some(tx)),
    });
    let app = Router::new()
        .route("/", get(index))
        .route("/install", post(install))
        .with_state(state);
    let url = format!("http://127.0.0.1:{}/", addr.port());
    println!("Verdict setup: {url}");
    let _ = std::io::stdout().flush();
    if opts.open {
        open_url(&url);
    }
    listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .map_err(std::io::Error::other)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "vdt-setup-ready-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn readiness_requires_this_app_this_project_and_this_pid() {
        let dir = temp_dir("ok");
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let page = crate::start_background(dir.clone(), 0).unwrap();
        let port = page.addr.port();
        // The page runs in this test process, so its pid is this one.
        assert!(
            wait_until_ready(&dir, port, std::process::id(), Duration::from_secs(5)).is_ok(),
            "this app, this project and this pid must read as ready"
        );
        // Same app, same project, but not the process the installer started.
        let err = wait_until_ready(
            &dir,
            port,
            std::process::id() + 1,
            Duration::from_millis(500),
        )
        .expect_err("a different pid is not the instance we spawned");
        assert!(err.contains("did not come up"), "{err}");
        // Same app and pid, but a different project.
        let other = temp_dir("other");
        let err = wait_until_ready(&other, port, std::process::id(), Duration::from_millis(500))
            .expect_err("a page for a different project is not ready");
        assert!(err.contains("did not come up"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn the_chosen_project_is_remembered_atomically() {
        let state = temp_dir("state");
        let project = temp_dir("project");
        record_last_project_in(&state, &project).unwrap();
        let want = std::fs::canonicalize(&project).unwrap();
        assert_eq!(last_project_in(&state), Some(want.clone()));
        let file = std::fs::read_to_string(state.join("last-project")).unwrap();
        assert_eq!(file.trim(), want.display().to_string(), "one absolute path");
        // Rewriting replaces the old choice rather than appending to it.
        let other = temp_dir("project2");
        record_last_project_in(&state, &other).unwrap();
        assert_eq!(
            last_project_in(&state),
            Some(std::fs::canonicalize(&other).unwrap())
        );
        // No temporary file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&state)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("last-project.tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        // A recorded folder that no longer exists is not opened.
        std::fs::remove_dir_all(&other).unwrap();
        assert_eq!(last_project_in(&state), None);
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn the_installer_opens_the_one_time_link_the_page_printed() {
        let printed =
            "Verdict page (open this link once): http://127.0.0.1:4731/?k=abc\nProject: x\n";
        assert_eq!(
            read_link(printed.as_bytes(), 4731).unwrap(),
            "http://127.0.0.1:4731/?k=abc"
        );
        // Another port's link, or no link at all, is not opened.
        assert!(read_link(printed.as_bytes(), 4732).is_err());
        assert!(read_link("Project: x\n".as_bytes(), 4731).is_err());
    }

    #[test]
    fn the_serve_argv_carries_the_chosen_port() {
        let args = serve_args(std::path::Path::new("C:/proj"), 4731);
        assert_eq!(args[0], "serve");
        assert!(
            args.windows(2).any(|w| w == ["--project", "C:/proj"]),
            "{args:?}"
        );
        // The installed server gets its own free port; the wizard keeps the page port.
        let wizard_port = 4733u16;
        let server_port = free_port().unwrap();
        assert_ne!(server_port, wizard_port);
        let args = serve_args(std::path::Path::new("C:/proj"), server_port);
        assert!(
            args.windows(2)
                .any(|w| w == ["--port", server_port.to_string().as_str()]),
            "{args:?}"
        );
        assert_ne!(server_port.to_string(), wizard_port.to_string());
        assert_ne!(args.last().unwrap(), &crate::DEFAULT_PORT.to_string());
    }

    #[test]
    fn the_open_url_uses_the_port_the_installer_bound() {
        // Not the old hard-coded 4717: the URL follows the port actually chosen (including a
        // free port picked from 0).
        assert_eq!(page_url(51234), "http://127.0.0.1:51234/");
        assert_ne!(page_url(51234), page_url(crate::DEFAULT_PORT));
    }

    #[test]
    fn nothing_listening_is_a_clear_failure_not_a_success() {
        let dir = temp_dir("closed");
        let closed = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let err = wait_until_ready(&dir, port, std::process::id(), Duration::from_millis(400))
            .expect_err("nothing answered, so the page is not ready");
        assert!(err.contains("cannot connect"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
