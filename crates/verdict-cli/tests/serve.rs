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
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
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
    let _ = std::fs::remove_dir_all(&dir);
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    assert!(raw.contains("Sessions"));
    assert!(raw.contains("Install hooks in this project"));
}
