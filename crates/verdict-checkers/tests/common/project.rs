//! A hand-made project for the tests that need something the fixtures do not hold.
#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;
use verdict_checkers::CheckContext;
use verdict_core::{
    ClaimSource, Lane, Ledger, Policy, Record, SessionParams, Writer, start_session,
};

use super::{approve_exec, init_repo};

pub struct Project {
    pub dir: TempDir,
    pub home: TempDir,
    pub ledger: Ledger,
    pub session: String,
}

impl Project {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Write (or overwrite) a file under the project root.
    pub fn write(&self, rel: &str, text: &str) {
        let p = self.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    pub fn claim(&mut self, source: ClaimSource, text: &str) {
        let names = verdict_core::part1c::extract_names(text);
        self.ledger
            .append(
                Writer::Agent,
                Some(&self.session),
                Record::Claim {
                    source,
                    text: text.into(),
                    names,
                    prompt_ordinal: None,
                },
            )
            .unwrap();
    }

    pub fn prompt(&mut self, text: &str) {
        self.ledger
            .append(
                Writer::Adapter,
                Some(&self.session),
                Record::Prompt {
                    ordinal: 1,
                    text: text.into(),
                    agent: "claude".into(),
                    model: "m".into(),
                },
            )
            .unwrap();
    }

    pub fn approve(&mut self) {
        let s = self.session.clone();
        approve_exec(&mut self.ledger, &s, self.dir.path(), self.home.path());
    }

    pub fn context(&self, lane: Lane, timeout: Duration) -> CheckContext {
        let policy = Policy::load(self.root(), Some(self.home.path())).unwrap();
        CheckContext::from_ledger(
            &self.ledger,
            self.root(),
            &self.session,
            lane,
            timeout,
            policy,
        )
        .unwrap()
    }
}

/// Make a project from `files`, commit them as the baseline when `git` is true, and open a
/// session on it.
pub fn project(files: &[(&str, &str)], git: bool) -> Project {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    if git {
        init_repo(dir.path());
    }
    let mut ledger = Ledger::open_in_memory().unwrap();
    let session = start_session(
        &mut ledger,
        dir.path(),
        &SessionParams {
            agent: "claude".into(),
            model: "m".into(),
            vendor: "anthropic".into(),
            home: Some(home.path().to_path_buf()),
        },
    )
    .unwrap()
    .id;
    Project {
        dir,
        home,
        ledger,
        session,
    }
}
