//! End-to-end tests driving the real `ticketsplease` binary against temp repos.

use std::path::Path;
use std::process::Command as Proc;

use assert_cmd::Command;
use tempfile::TempDir;

fn tkt(repo: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ticketsplease").unwrap();
    cmd.arg("--repo").arg(repo);
    cmd
}

fn ready_ids(repo: &Path) -> Vec<String> {
    let out = tkt(repo)
        .args(["ready", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["ready"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn crud_scheduling_and_exit_codes() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args([
            "create", "--id", "base", "--title", "Base", "--scope", "core",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "dep",
            "--title",
            "Dep",
            "--scope",
            "io",
            "--depends-on",
            "base",
        ])
        .assert()
        .success();

    // `dep` is blocked until `base` is done.
    assert_eq!(ready_ids(repo), vec!["base"]);
    tkt(repo)
        .args(["set", "base", "--status", "done"])
        .assert()
        .success();
    assert_eq!(ready_ids(repo), vec!["dep"]);

    tkt(repo).args(["lint"]).assert().success();

    // Exit-code contract.
    tkt(repo).args(["show", "ghost"]).assert().code(4); // not found
    tkt(repo)
        .args(["create", "--title", "X", "--status", "bogus"])
        .assert()
        .code(3); // invalid

    // A cycle makes scheduling fail with code 5.
    tkt(repo)
        .args(["create", "--id", "x", "--title", "X", "--scope", "core"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "y", "--title", "Y", "--scope", "core"])
        .assert()
        .success();
    tkt(repo)
        .args(["link", "x", "--depends-on", "y"])
        .assert()
        .success();
    tkt(repo)
        .args(["link", "y", "--depends-on", "x"])
        .assert()
        .success();
    tkt(repo).args(["ready"]).assert().code(5);
}

#[test]
fn guard_flags_under_declaration_then_clears() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    std::fs::write(
        repo.join("ticketsplease.toml"),
        "schema_version = 1\ntickets_dir = \"tickets\"\ndefault_base = \"main\"\n\
         [language]\nbackend = \"none\"\n[scopes]\n\"core\" = [\"core/**\"]\n\"io\" = [\"io/**\"]\n",
    )
    .unwrap();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T", "--scope", "core"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "t", "--status", "in-progress"])
        .assert()
        .success();

    // Git fixture: main has both dirs; the branch edits io/ (undeclared).
    std::fs::create_dir_all(repo.join("core")).unwrap();
    std::fs::create_dir_all(repo.join("io")).unwrap();
    std::fs::write(repo.join("core/a.txt"), "a\n").unwrap();
    std::fs::write(repo.join("io/b.txt"), "b\n").unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["add", "-A"]);
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ],
    );
    git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("io/b.txt"), "changed\n").unwrap();
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qam",
            "edit",
        ],
    );

    // Branch touched `io`, ticket declared only `core` -> conflict (exit 6).
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t"])
        .assert()
        .code(6);

    // Declare `io` -> clean (exit 0).
    tkt(repo)
        .args(["set", "t", "--add-scope", "io"])
        .assert()
        .success();
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t"])
        .assert()
        .success();
}

#[test]
fn json_output_is_byte_deterministic() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A", "--scope", "x"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B", "--scope", "y"])
        .assert()
        .success();

    let run = || {
        tkt(repo)
            .args(["tracks", "--format", "json"])
            .output()
            .unwrap()
            .stdout
    };
    assert_eq!(
        run(),
        run(),
        "json output must be byte-identical across runs (R13)"
    );
}

#[test]
fn set_updates_body() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "b",
            "--title",
            "B",
            "--body",
            "original body",
        ])
        .assert()
        .success();

    tkt(repo)
        .args(["set", "b", "--body", "replaced body"])
        .assert()
        .success();
    let out = tkt(repo).args(["show", "b"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("replaced body") && !text.contains("original body"));
    assert!(text.contains("id: b"), "frontmatter must be preserved");

    tkt(repo)
        .args(["set", "b", "--append-body", "- a note"])
        .assert()
        .success();
    let out = tkt(repo).args(["show", "b"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("replaced body"));
    assert!(text.contains("- a note"));
}

#[test]
fn set_body_from_file_and_remove_tag() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "f", "--title", "F", "--tag", "keep,drop"])
        .assert()
        .success();

    // Rich body with shell-hostile content, supplied via a file (no shell interpolation).
    let body_path = repo.join("body.md");
    std::fs::write(
        &body_path,
        "Spec with `record_dml_predicate` and $(danger).\n",
    )
    .unwrap();
    tkt(repo)
        .args(["set", "f", "--body-file", body_path.to_str().unwrap()])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "f", "--remove-tag", "drop"])
        .assert()
        .success();

    let out = tkt(repo).args(["show", "f"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("`record_dml_predicate`"));
    assert!(text.contains("$(danger)"));
    assert!(
        text.contains("tags: [keep]"),
        "remove-tag should leave [keep]"
    );
}

#[test]
fn create_from_batch() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();

    let specs = r#"[
      {"id":"a","title":"A","priority":"p1","scopes":["core"]},
      {"id":"b","title":"B","depends_on":["a"],"scopes":["io"],"body":"spec for b"}
    ]"#;
    let path = repo.join("backlog.json");
    std::fs::write(&path, specs).unwrap();
    tkt(repo)
        .args(["create", "--from", path.to_str().unwrap()])
        .assert()
        .success();

    let out = tkt(repo)
        .args(["list", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"a") && ids.contains(&"b"));

    let show = tkt(repo).args(["show", "b"]).output().unwrap();
    let text = String::from_utf8(show.stdout).unwrap();
    assert!(text.contains("spec for b"));
    assert!(text.contains("dependencies: [a]"));

    // Explicit ids make a re-run idempotent (no error, no duplicates).
    tkt(repo)
        .args(["create", "--from", path.to_str().unwrap()])
        .assert()
        .success();
}

/// Make `repo` a git repo with one commit — the claim lock ref targets HEAD.
fn git_init_commit(repo: &Path) {
    git(repo, &["init", "-q", "-b", "main"]);
    git(
        repo,
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
    );
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ],
    );
}

#[test]
fn claim_release_and_steal() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);

    // alice claims -> in-progress -> excluded from the ready pool.
    tkt(repo)
        .args(["claim", "t", "--as", "alice"])
        .assert()
        .success();
    assert!(ready_ids(repo).is_empty(), "a claimed ticket is not ready");

    // A live claim blocks others, and a non-holder cannot release it.
    tkt(repo)
        .args(["claim", "t", "--as", "bob"])
        .assert()
        .code(6);
    tkt(repo)
        .args(["release", "t", "--as", "bob"])
        .assert()
        .code(6);

    // The holder releases; bob then claims cleanly.
    tkt(repo)
        .args(["release", "t", "--as", "alice"])
        .assert()
        .success();
    tkt(repo)
        .args(["claim", "t", "--as", "bob"])
        .assert()
        .success();
    let show = tkt(repo)
        .args(["show", "t", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(v["assignee"], "bob");

    // An expired lease (ttl 0) is reclaimable: carol takes it over.
    tkt(repo)
        .args(["release", "t", "--as", "bob"])
        .assert()
        .success();
    tkt(repo)
        .args(["claim", "t", "--as", "dave", "--ttl", "0"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["claim", "t", "--as", "carol", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["assignee"], "carol");
    assert_eq!(v["stolen"], true, "an expired lease should be stolen");
}

#[test]
fn concurrent_claims_have_exactly_one_winner() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().to_path_buf();
    tkt(&repo).args(["init", "--no-skill"]).assert().success();
    tkt(&repo)
        .args(["create", "--id", "hot", "--title", "Hot"])
        .assert()
        .success();
    git_init_commit(&repo);

    // Race many agents at one ticket; git's create-only ref update must let exactly
    // one win and turn every loser into a clean exit-6 conflict (never a co-winner).
    let bin = env!("CARGO_BIN_EXE_ticketsplease");
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let repo = repo.clone();
            let bin = bin.to_string();
            std::thread::spawn(move || {
                Proc::new(&bin)
                    .arg("--repo")
                    .arg(&repo)
                    .args(["claim", "hot", "--as", &format!("racer{i}")])
                    .output()
                    .unwrap()
                    .status
                    .code()
                    .unwrap_or(-1)
            })
        })
        .collect();
    let codes: Vec<i32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let winners = codes.iter().filter(|&&c| c == 0).count();
    let conflicts = codes.iter().filter(|&&c| c == 6).count();
    assert_eq!(winners, 1, "exactly one claimer must win; got {codes:?}");
    assert_eq!(
        winners + conflicts,
        codes.len(),
        "every loser must be a clean conflict (exit 6); got {codes:?}"
    );
}

fn git(repo: &Path, args: &[&str]) {
    let status = Proc::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}
