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
            "<p>Nothing was installed: {}</p><p><a href=\"/\">Back</a></p>",
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
        verdict_adapters::mcp_command(&req, project, &mut out)
            .map_err(|e| format!("could not register {}: {e}", host.title()))?;
    }
    if open_page {
        launcher(project, &target)?;
        open_project_page(project, &target);
    }
    Ok(Installed { vdt: target })
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
             $s.Arguments='serve --project \"{}\"';$s.WorkingDirectory='{}';$s.Save()",
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
             Exec=\"{}\" serve --project \"{}\"\nPath={}\nTerminal=false\nCategories=Development;\n",
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

fn open_project_page(project: &Path, vdt: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new(vdt)
            .args(["serve", "--project"])
            .arg(project)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new(vdt)
            .args(["serve", "--project"])
            .arg(project)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    std::thread::sleep(std::time::Duration::from_millis(600));
    open_url("http://127.0.0.1:4717/");
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
