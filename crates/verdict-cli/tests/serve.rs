//! `vdt serve` as a real process: it listens on 127.0.0.1 and answers.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use verdict_core::*;

#[test]
fn vdt_serve_listens_on_loopback_and_serves_the_sessions_page() {
    let dir = std::env::temp_dir().join(format!("vdt-serve-test-{}", std::process::id()));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    {
        let mut l = Ledger::open(&dir).unwrap();
        start_session(
            &mut l,
            &dir,
            &SessionParams {
                agent: "claude".into(),
                model: "m".into(),
                vendor: "anthropic".into(),
                home: Some(home),
            },
        )
        .unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_vdt"))
        .args(["serve", "--port", "0", "--project"])
        .arg(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut first = String::new();
    // Keep the reader (and so the pipe) open until the request is done.
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    stdout.read_line(&mut first).unwrap();
    let url = first.trim().strip_prefix("Verdict page: ").expect(&first);
    let hostport = url
        .strip_prefix("http://")
        .unwrap()
        .trim_end_matches('/')
        .to_string();
    assert!(hostport.starts_with("127.0.0.1:"), "{url}");

    let mut s = TcpStream::connect(&hostport).unwrap();
    write!(
        s,
        "GET / HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    drop(stdout);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    assert!(raw.contains("Sessions"));
    assert!(raw.contains("Install hooks in this project"));
}

#[test]
fn vdt_serve_keeps_running_when_nobody_reads_its_output() {
    let dir = std::env::temp_dir().join(format!("vdt-serve-closed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    drop(Ledger::open(&dir).unwrap());
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vdt"))
        .args(["serve", "--port", &port.to_string(), "--project"])
        .arg(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Close the read end at once: every line the page prints goes to a closed pipe.
    drop(child.stdout.take());
    let hostport = format!("127.0.0.1:{port}");
    let mut raw = String::new();
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        let Ok(mut s) = TcpStream::connect(&hostport) else {
            continue;
        };
        let _ = write!(
            s,
            "GET / HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n"
        );
        if s.read_to_string(&mut raw).is_ok() && !raw.is_empty() {
            break;
        }
    }
    let exited = child.try_wait().unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(exited.is_none(), "the page stopped: {exited:?}");
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
}
