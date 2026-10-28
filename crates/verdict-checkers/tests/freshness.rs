//! F03: a check's snapshot stamp names the tree the checker actually read, not a later one.

use std::time::Duration;

use verdict_checkers::{RunOptions, run_lane_with_home};
use verdict_core::{Lane, Ledger, Record, SessionParams, baseline, start_session};

/// The snapshot stamp of the latest check record with this name.
fn stamp_of(ledger: &Ledger, checker: &str) -> Option<String> {
    ledger
        .records()
        .unwrap()
        .into_iter()
        .rev()
        .find_map(|s| match &s.record {
            Record::Check {
                name, tree_state, ..
            } if name == checker => Some(tree_state.clone()),
            _ => None,
        })
        .flatten()
}

#[test]
fn a_check_is_stamped_with_the_tree_it_read_and_the_stamp_moves_with_the_tree() {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.ts"), "export const a = 1;\n").unwrap();

    let mut ledger = Ledger::open(dir.path()).unwrap();
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

    let opts = RunOptions {
        lane: Lane::Low,
        timeout: Duration::from_secs(10),
        deadline: None,
        force: false,
    };
    run_lane_with_home(
        &mut ledger,
        dir.path(),
        &session,
        &["regressions".to_string()],
        &opts,
        Some(home.path()),
    )
    .unwrap();

    let first = stamp_of(&ledger, "regressions").expect("a file-backed run must stamp the tree");
    // The stamp is the state of the source the checker read, so the verdict's comparison with the
    // tree now can say this check is about the current code.
    assert_eq!(Some(first.clone()), baseline::tree_state(dir.path()));

    // A change to the source moves the stamp, so evidence about the old content can never be
    // mistaken for evidence about the new content.
    std::fs::write(dir.path().join("src/a.ts"), "export const a = 2;\n").unwrap();
    run_lane_with_home(
        &mut ledger,
        dir.path(),
        &session,
        &["regressions".to_string()],
        &opts,
        Some(home.path()),
    )
    .unwrap();
    let second = stamp_of(&ledger, "regressions").expect("the second run is stamped too");
    assert_ne!(first, second);
    assert_eq!(Some(second), baseline::tree_state(dir.path()));
}
