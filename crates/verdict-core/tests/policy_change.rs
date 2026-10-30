mod p2b_common;

use p2b_common::*;
use verdict_core::human::{Action, apply};
use verdict_core::verdict::compute_live;

fn mentions_rule_change(lines: &[String]) -> bool {
    lines.iter().any(|l| l.contains("the rules changed"))
}

#[test]
fn editing_the_rules_mid_session_needs_the_human_until_they_accept_it() {
    let repo = git_repo("policychange");
    let home = Tmp::new("home");
    std::fs::write(repo.path().join("verdict.json"), r#"{"version":1}"#).unwrap();
    git(repo.path(), &["add", "verdict.json"]);
    git(repo.path(), &["commit", "-q", "-m", "policy"]);
    let (mut l, sid) = repo_ledger(repo.path(), home.path());

    // No change yet: nothing to settle about the rules, and nothing to accept.
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert!(!mentions_rule_change(&c.yours_to_settle));
    let refused = apply(
        repo.path(),
        &mut l,
        &sid,
        &Action::AcceptPolicy,
        Some(home.path()),
    );
    assert!(refused.is_err(), "accepting unchanged rules is refused");

    // The agent edits verdict.json during the session.
    std::fs::write(
        repo.path().join("verdict.json"),
        r#"{"version":1,"blast":{"high_if_touches":["src/**"]}}"#,
    )
    .unwrap();
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert!(
        mentions_rule_change(&c.yours_to_settle),
        "the change is the human's to settle: {:?}",
        c.yours_to_settle
    );
    assert!(
        !matches!(
            c.word,
            verdict_core::VerdictWord::Verified | verdict_core::VerdictWord::ClearToShip
        ),
        "a rule change never reads as verified"
    );

    // The human accepts the new rules: the rule-change line is gone.
    apply(
        repo.path(),
        &mut l,
        &sid,
        &Action::AcceptPolicy,
        Some(home.path()),
    )
    .unwrap();
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert!(!mentions_rule_change(&c.yours_to_settle));

    // A second, different change needs a second acceptance.
    std::fs::write(
        repo.path().join("verdict.json"),
        r#"{"version":1,"blast":{"high_if_touches":["lib/**"]}}"#,
    )
    .unwrap();
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert!(mentions_rule_change(&c.yours_to_settle));

    // Putting the original rules back also clears it.
    std::fs::write(repo.path().join("verdict.json"), r#"{"version":1}"#).unwrap();
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert!(!mentions_rule_change(&c.yours_to_settle));
}
