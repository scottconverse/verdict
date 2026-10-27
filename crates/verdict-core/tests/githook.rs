mod p2b_common;

use p2b_common::*;
use verdict_core::githook::*;
use verdict_core::*;

fn req(repo: &Tmp, install: bool, human: bool) -> HookRequest {
    HookRequest {
        install,
        project: repo.path().to_path_buf(),
        vdt_path: Some("C:\\tools\\vdt.exe".into()),
        i_am_the_human: human,
    }
}

fn run(r: &HookRequest) -> (u8, String) {
    let mut out = Vec::new();
    let code = hook_command(r, &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

fn hook_path(repo: &Tmp) -> std::path::PathBuf {
    hooks_dir(repo.path()).unwrap().join("pre-commit")
}

fn decisions(repo: &Tmp) -> Vec<(Writer, DecisionKind, String)> {
    match Ledger::open_existing(repo.path()).unwrap() {
        None => vec![],
        Some(l) => l
            .records()
            .unwrap()
            .into_iter()
            .filter_map(|s| match s.record {
                Record::Decision {
                    decision, choice, ..
                } => Some((s.writer, decision, choice)),
                _ => None,
            })
            .collect(),
    }
}

#[test]
fn without_the_human_flag_it_prints_what_it_would_write_and_writes_nothing() {
    let repo = git_repo("hook-dry");
    let (code, text) = run(&req(&repo, true, false));
    assert_eq!(code, 0);
    assert!(text.contains("Nothing was changed"), "{text}");
    assert!(text.contains("--i-am-the-human"));
    assert!(
        text.contains("#!/bin/sh") && text.contains("gate pre-commit"),
        "{text}"
    );
    assert!(!hook_path(&repo).exists());
    assert!(decisions(&repo).is_empty());
    assert!(
        !repo.path().join(".verdict").exists(),
        "no ledger was created"
    );
    // Same for uninstall.
    let (code, text) = run(&req(&repo, false, false));
    assert_eq!(code, 0);
    assert!(text.contains("Nothing was changed"));
}

#[test]
fn install_writes_a_fail_open_sh_hook_with_forward_slash_paths_and_a_human_decision() {
    let repo = git_repo("hook-install");
    let (code, text) = run(&req(&repo, true, true));
    assert_eq!(code, 0, "{text}");
    let body = std::fs::read_to_string(hook_path(&repo)).unwrap();
    assert!(body.starts_with("#!/bin/sh\n"), "{body}");
    assert!(body.contains(MARKER));
    assert!(body.contains("'C:/tools/vdt.exe'"), "{body}");
    assert!(!body.contains('\r'), "an sh script must have LF endings");
    let d = decisions(&repo);
    assert_eq!(d.len(), 1);
    assert_eq!(
        (d[0].0, d[0].1),
        (Writer::Human, DecisionKind::InstallHooks)
    );
    assert!(d[0].2.contains("install git pre-commit gate"));
    // Installing again is a no-op in effect (same file), not a second hook.
    let (code, _) = run(&req(&repo, true, true));
    assert_eq!(code, 0);
    assert_eq!(std::fs::read_to_string(hook_path(&repo)).unwrap(), body);
    assert!(!hooks_dir(repo.path()).unwrap().join(CHAINED).exists());
}

#[test]
fn an_existing_hook_is_kept_by_chaining_with_a_backup_and_restored_on_uninstall() {
    let repo = git_repo("hook-chain");
    let hooks = hooks_dir(repo.path()).unwrap();
    std::fs::create_dir_all(&hooks).unwrap();
    let other = "#!/bin/sh\necho other tool\nexit 0\n";
    std::fs::write(hooks.join("pre-commit"), other).unwrap();

    let (code, text) = run(&req(&repo, true, true));
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("kept the existing hook"), "{text}");
    assert_eq!(std::fs::read_to_string(hooks.join(CHAINED)).unwrap(), other);
    assert_eq!(std::fs::read_to_string(hooks.join(BACKUP)).unwrap(), other);
    let ours = std::fs::read_to_string(hooks.join("pre-commit")).unwrap();
    assert!(ours.contains(MARKER) && ours.contains(CHAINED));

    // Uninstall: ours goes, theirs comes back, the backup stays.
    let (code, text) = run(&req(&repo, false, true));
    assert_eq!(code, 0, "{text}");
    assert_eq!(
        std::fs::read_to_string(hooks.join("pre-commit")).unwrap(),
        other
    );
    assert!(!hooks.join(CHAINED).exists());
    assert_eq!(std::fs::read_to_string(hooks.join(BACKUP)).unwrap(), other);
    let d = decisions(&repo);
    assert_eq!(d.len(), 2);
    assert!(d[1].2.contains("uninstall"));
}

#[test]
fn uninstall_never_deletes_another_tools_hook() {
    let repo = git_repo("hook-theirs");
    let hooks = hooks_dir(repo.path()).unwrap();
    std::fs::create_dir_all(&hooks).unwrap();
    let other = "#!/bin/sh\necho husky\n";
    std::fs::write(hooks.join("pre-commit"), other).unwrap();
    let (code, text) = run(&req(&repo, false, true));
    assert_eq!(code, 0);
    assert!(text.contains("not Verdict's hook"), "{text}");
    assert_eq!(
        std::fs::read_to_string(hooks.join("pre-commit")).unwrap(),
        other
    );
    assert!(
        decisions(&repo).is_empty(),
        "nothing changed, so no decision"
    );
    // Uninstall with no hook at all is also a quiet no-op.
    std::fs::remove_file(hooks.join("pre-commit")).unwrap();
    let (code, text) = run(&req(&repo, false, true));
    assert_eq!(code, 0);
    assert!(text.contains("nothing changed"), "{text}");
}

#[test]
fn a_second_backup_never_overwrites_the_first() {
    let repo = git_repo("hook-backup2");
    let hooks = hooks_dir(repo.path()).unwrap();
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join(BACKUP), "an older backup\n").unwrap();
    std::fs::write(hooks.join("pre-commit"), "#!/bin/sh\necho newer\n").unwrap();
    let (code, _) = run(&req(&repo, true, true));
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(hooks.join(BACKUP)).unwrap(),
        "an older backup\n"
    );
    assert_eq!(
        std::fs::read_to_string(hooks.join(format!("{BACKUP}-2"))).unwrap(),
        "#!/bin/sh\necho newer\n"
    );
}

#[test]
fn outside_a_git_repository_it_says_so() {
    let plain = Tmp::new("hook-plain");
    let r = HookRequest {
        install: true,
        project: plain.path().to_path_buf(),
        vdt_path: None,
        i_am_the_human: true,
    };
    let mut out = Vec::new();
    let e = hook_command(&r, &mut out).unwrap_err();
    assert!(e.contains("not a git repository"), "{e}");
}

#[test]
fn a_quote_in_the_vdt_path_cannot_break_out_of_the_script() {
    let s = hook_script("C:\\it's here\\vdt.exe");
    assert!(s.contains("VDT='C:/it'\\''s here/vdt.exe'"), "{s}");
}
