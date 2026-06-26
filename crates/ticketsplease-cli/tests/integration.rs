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

/// Write a minimal 2-crate cargo workspace (crate-b depends on crate-a) into `repo`.
fn write_cargo_fixture(repo: &Path) {
    std::fs::write(
        repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crate-a\", \"crate-b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(repo.join("crate-a/src")).unwrap();
    std::fs::write(
        repo.join("crate-a/Cargo.toml"),
        "[package]\nname = \"crate-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(repo.join("crate-a/src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::create_dir_all(repo.join("crate-b/src")).unwrap();
    std::fs::write(
        repo.join("crate-b/Cargo.toml"),
        "[package]\nname = \"crate-b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ncrate-a = { path = \"../crate-a\" }\n",
    )
    .unwrap();
    std::fs::write(repo.join("crate-b/src/lib.rs"), "pub fn b() {}\n").unwrap();
}

/// The reverse-dependency walk: editing crate-a (a leaf) flags crate-b (a
/// dependent) transitively. `--direct-only` suppresses that expansion.
#[test]
fn guard_cargo_reverse_dep_is_tagged_transitive() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_cargo_fixture(repo);
    std::fs::write(
        repo.join("ticketsplease.toml"),
        "schema_version = 1\ntickets_dir = \"tickets\"\ndefault_base = \"main\"\n\
         [language]\nbackend = \"rust\"\n[scopes]\n[scope_crates]\n\"a\" = \"crate-a\"\n\"b\" = \"crate-b\"\n",
    )
    .unwrap();
    // `t` owns crate-a (declares scope a + b to isolate the collision), `u` owns crate-b.
    tkt(repo)
        .args([
            "create", "--id", "t", "--title", "T", "--scope", "a", "--scope", "b",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "t", "--status", "in-progress"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "u", "--title", "U", "--scope", "b"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "u", "--status", "in-progress"])
        .assert()
        .success();

    git_init_commit(repo);
    git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(
        repo.join("crate-a/src/lib.rs"),
        "pub fn a() { /* edit */ }\n",
    )
    .unwrap();
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qam",
            "edit a",
        ],
    );

    // Default: crate-b is reached only via reverse-deps -> transitive collision, exit 6.
    let out = tkt(repo)
        .args(["guard", "feat", "--ticket", "t", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(6),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["affected_causes"]["a"], "direct");
    assert_eq!(v["affected_causes"]["b"], "transitive");
    assert_eq!(v["collisions"][0]["ticket"], "u");
    assert_eq!(v["collisions"][0]["cause"], "transitive");

    // --direct-only drops the reverse-dep expansion: no collision, clean.
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t", "--direct-only"])
        .assert()
        .success();
    // The alias resolves to the same behaviour.
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t", "--no-reverse-deps"])
        .assert()
        .success();
}

const CARGO_PIN: &str = "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n\
     [dependencies]\nsqlparser = { git = \"https://github.com/example/sqlparser\", rev = \"REV\" }\n";

/// A branch that bumps a pinned external `git`/`rev` dependency is flagged against
/// the matching external scope, even with the language backend off.
#[test]
fn guard_flags_external_scope_rev_bump() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    std::fs::write(
        repo.join("ticketsplease.toml"),
        "schema_version = 1\ntickets_dir = \"tickets\"\ndefault_base = \"main\"\n\
         [language]\nbackend = \"none\"\n\
         [external_scopes]\n\"sqlparser-fork\" = { repo = \"example/sqlparser\" }\n",
    )
    .unwrap();
    std::fs::write(repo.join("Cargo.toml"), CARGO_PIN.replace("REV", "aaaaaaa")).unwrap();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();

    git_init_commit(repo);
    git(repo, &["checkout", "-q", "-b", "feat"]);
    // The only change on the branch: bump the pinned rev.
    std::fs::write(repo.join("Cargo.toml"), CARGO_PIN.replace("REV", "bbbbbbb")).unwrap();
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qam",
            "bump rev",
        ],
    );

    // Undeclared external scope -> exit 6, tagged direct.
    let out = tkt(repo)
        .args(["guard", "feat", "--ticket", "t", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(6),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["affected_causes"]["sqlparser-fork"], "direct");
    assert_eq!(v["under_declared"][0], "sqlparser-fork");

    // Declaring the external scope clears the gate.
    tkt(repo)
        .args(["set", "t", "--add-scope", "sqlparser-fork"])
        .assert()
        .success();
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t"])
        .assert()
        .success();
}

/// Two tickets declaring the same external scope name never share a `tracks` batch
/// — the scheduler treats external scopes like any other named scope.
#[test]
fn tracks_separates_tickets_sharing_external_scope() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "p",
            "--title",
            "P",
            "--scope",
            "sqlparser-fork",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "q",
            "--title",
            "Q",
            "--scope",
            "sqlparser-fork",
        ])
        .assert()
        .success();

    let out = tkt(repo)
        .args(["tracks", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let batches = v["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 2, "conflicting tickets need two batches");
    for b in batches {
        let ids: Vec<&str> = b
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert!(
            !(ids.contains(&"p") && ids.contains(&"q")),
            "p and q share a scope; must not share a batch"
        );
    }
}

/// `show --ref` and `status --all-branches` observe a worker's in-flight status
/// committed on its branch, while the working tree (main) still shows the old one.
#[test]
fn cross_branch_state_is_observable() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);

    // Branch tkt/t advances the ticket to review, committed on the branch only.
    git(repo, &["checkout", "-q", "-b", "tkt/t"]);
    tkt(repo)
        .args(["set", "t", "--status", "review"])
        .assert()
        .success();
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qam",
            "review",
        ],
    );
    git(repo, &["checkout", "-q", "main"]);

    // Working tree (main) still shows todo; the branch tip shows review.
    let wt: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["show", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(wt["status"], "todo");
    let on_branch: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["show", "t", "--ref", "tkt/t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(on_branch["status"], "review");

    // status --all-branches reports the branch tip status from main.
    let st: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["status", "--all-branches", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(st["source"], "branches");
    let row = st["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["branch"] == "tkt/t")
        .expect("tkt/t row present");
    assert_eq!(row["status"], "review");
    assert_eq!(row["id"], "t");
}

/// `watch` returns 0 immediately when the ticket is already at the target.
#[test]
fn watch_returns_when_already_at_target() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "t", "--status", "review"])
        .assert()
        .success();

    // No git branch -> polls the working tree, already at review.
    let out = tkt(repo)
        .args(["watch", "t", "--until", "review", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["reached"], true);
    assert_eq!(v["status"], "review");
}

/// `watch` exits 7 (timeout) when the target is never reached.
#[test]
fn watch_times_out_with_exit_7() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();

    let out = tkt(repo)
        .args([
            "watch",
            "t",
            "--until",
            "review",
            "--timeout",
            "1",
            "--interval",
            "1",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(7),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["timed_out"], true);
    assert_eq!(v["reached"], false);
}

/// `watch` with no `--ref` auto-resolves the conventional `tkt/<id>` branch and
/// polls it — so an orchestrator on `main` sees the worker reach `review`.
#[test]
fn watch_auto_resolves_the_ticket_branch() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);

    // Worker advances the ticket to review on its branch, then we return to main.
    git(repo, &["checkout", "-q", "-b", "tkt/t"]);
    tkt(repo)
        .args(["set", "t", "--status", "review"])
        .assert()
        .success();
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qam",
            "review",
        ],
    );
    git(repo, &["checkout", "-q", "main"]);

    // No --ref: resolves tkt/t (exists) and reads review off its tip.
    let out = tkt(repo)
        .args(["watch", "t", "--until", "review", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["reached"], true);
    assert_eq!(v["ref"], "tkt/t");
    assert_eq!(v["status"], "review");
}

/// Comments: add (inline + shell-safe stdin), list, fold into show, and a missing
/// ticket is not-found.
#[test]
fn comments_add_list_and_show() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();

    tkt(repo)
        .args(["comment", "add", "t", "--as", "w1", "--body", "first note"])
        .assert()
        .success();
    // Shell-hostile content via stdin (`--body-file -`) — no shell interpolation.
    tkt(repo)
        .args(["comment", "add", "t", "--as", "w2", "--body-file", "-"])
        .write_stdin("second `note` with $(danger)")
        .assert()
        .success();

    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["comment", "list", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let cs = v["comments"].as_array().unwrap();
    assert_eq!(cs.len(), 2);
    assert_eq!(cs[0]["by"], "w1"); // sorted chronologically by id
    assert_eq!(cs[0]["body"], "first note");
    assert_eq!(cs[1]["by"], "w2");
    assert_eq!(cs[1]["body"], "second `note` with $(danger)");

    // show folds comments into its JSON.
    let shown: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["show", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(shown["comments"].as_array().unwrap().len(), 2);

    // Commenting on a missing ticket is not-found (exit 4).
    tkt(repo)
        .args(["comment", "add", "ghost", "--body", "x"])
        .assert()
        .code(4);
}

/// A worker's comments on its branch are readable from `main` via `--ref`.
#[test]
fn comments_are_readable_across_branches() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);

    git(repo, &["checkout", "-q", "-b", "tkt/t"]);
    tkt(repo)
        .args([
            "comment",
            "add",
            "t",
            "--as",
            "w1",
            "--body",
            "from the branch",
        ])
        .assert()
        .success();
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
            "comment",
        ],
    );
    git(repo, &["checkout", "-q", "main"]);

    // Working tree on main has no comments; --ref tkt/t sees it.
    let wt: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["comment", "list", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(wt["comments"].as_array().unwrap().len(), 0);

    let on_ref: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["comment", "list", "t", "--ref", "tkt/t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let cs = on_ref["comments"].as_array().unwrap();
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0]["body"], "from the branch");
}

/// The conflict-free guarantee: 8 concurrent authors all land, none lost.
#[test]
fn concurrent_comments_are_all_kept() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();

    let bin = assert_cmd::cargo::cargo_bin("ticketsplease");
    let repo_path = repo.to_path_buf();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let bin = bin.clone();
            let repo = repo_path.clone();
            std::thread::spawn(move || {
                Proc::new(bin)
                    .arg("--repo")
                    .arg(&repo)
                    .args([
                        "comment",
                        "add",
                        "t",
                        "--as",
                        &format!("w{i}"),
                        "--body",
                        &format!("note {i}"),
                    ])
                    .status()
                    .unwrap()
                    .success()
            })
        })
        .collect();
    for h in handles {
        assert!(
            h.join().unwrap(),
            "each concurrent comment add must succeed"
        );
    }

    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["comment", "list", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        v["comments"].as_array().unwrap().len(),
        8,
        "all concurrent comments must survive (conflict-free)"
    );
}
