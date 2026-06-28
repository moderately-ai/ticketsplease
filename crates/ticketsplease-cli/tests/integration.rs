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

    // A dependency cycle is rejected at link write time (exit 5), not deferred to
    // scheduling.
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
    // Closing the loop would create x -> y -> x; reject it before it corrupts the graph.
    tkt(repo)
        .args(["link", "y", "--depends-on", "x"])
        .assert()
        .code(5);
    // The rejected edge was never persisted, so scheduling stays healthy.
    tkt(repo).args(["ready"]).assert().success();
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
    let raw = std::fs::read_to_string(repo.join("tickets/b.md")).unwrap();
    assert!(raw.contains("id: b"), "frontmatter must be preserved");

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
    let raw = std::fs::read_to_string(repo.join("tickets/f.md")).unwrap();
    assert!(
        raw.contains("tags: [keep]"),
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
    assert!(text.contains("deps:") && text.contains('a'));

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

    // --ignore-transitive passes the gate (the only conflict is transitive) but —
    // unlike --direct-only — keeps the transitive collision in the report for triage.
    let out = tkt(repo)
        .args([
            "guard",
            "feat",
            "--ticket",
            "t",
            "--ignore-transitive",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "transitive-only must pass with --ignore-transitive"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["transitive_only"], true);
    assert_eq!(
        v["collisions"][0]["cause"], "transitive",
        "the collision is still reported, not dropped"
    );
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

/// The event doorbell: `comment add` emits an event ref in `.git`, visible via
/// `tkt events` with no commit — and `--since` / `--ticket` / `--type` filter it.
#[test]
fn comment_emits_a_live_event_before_commit() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    // A git repo, but deliberately no commit — the event lives in .git refs.
    git(repo, &["init", "-q", "-b", "main"]);

    tkt(repo)
        .args(["comment", "add", "t", "--as", "w1", "--body", "live note"])
        .assert()
        .success();

    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["events", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let evs = v["events"].as_array().unwrap();
    assert_eq!(evs.len(), 1, "the comment event is visible with no commit");
    assert_eq!(evs[0]["kind"], "comment");
    assert_eq!(evs[0]["ticket"], "t");
    assert_eq!(evs[0]["by"], "w1");
    let first_id = evs[0]["id"].as_str().unwrap().to_string();

    // A second comment; --since the first cursor returns only the newer event.
    tkt(repo)
        .args(["comment", "add", "t", "--body", "second"])
        .assert()
        .success();
    let since: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["events", "--since", &first_id, "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        since["events"].as_array().unwrap().len(),
        1,
        "--since returns only events newer than the cursor"
    );

    // --type filters by kind; no status events have been emitted.
    let typed: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["events", "--type", "status", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(typed["events"].as_array().unwrap().len(), 0);

    // --ticket filters by ticket; both comment events are for `t`.
    let by_ticket: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["events", "--ticket", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(by_ticket["events"].as_array().unwrap().len(), 2);
}

/// claim / set --status / release each drop an event, so the log is a full feed.
#[test]
fn status_claim_release_emit_events() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo); // claim's ref-CAS needs HEAD to be a real commit

    tkt(repo)
        .args(["claim", "t", "--as", "w1"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "t", "--status", "review"])
        .assert()
        .success();
    tkt(repo)
        .args(["release", "t", "--as", "w1", "--force"])
        .assert()
        .success();

    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["events", "--ticket", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let kinds: Vec<&str> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"claim"), "claim event: {kinds:?}");
    assert!(kinds.contains(&"status"), "status event: {kinds:?}");
    assert!(kinds.contains(&"release"), "release event: {kinds:?}");
}

/// `events --watch` returns immediately when an event exists, and exits 7 on timeout.
#[test]
fn events_watch_wakes_and_times_out() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git(repo, &["init", "-q", "-b", "main"]);

    // Nothing yet: --watch with a short timeout exits 7 with an empty payload.
    let out = tkt(repo)
        .args([
            "events",
            "--watch",
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
    assert_eq!(v["events"].as_array().unwrap().len(), 0);

    // Emit one; --watch finds it on the first poll and exits 0.
    tkt(repo)
        .args(["comment", "add", "t", "--body", "x"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["events", "--watch", "--timeout", "5", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["events"].as_array().unwrap().len(), 1);
}

/// A foundational crate (`ast`) split into sub-crate scopes that all map to it,
/// depended on by `parser`. Mirrors the real bug report.
fn write_ast_workspace(repo: &Path) {
    std::fs::write(
        repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/ast\", \"crates/parser\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for d in [
        "crates/ast/src/dialect",
        "crates/ast/src/precedence",
        "crates/ast/src/vocab",
        "crates/ast/src/nodes",
        "crates/parser/src",
    ] {
        std::fs::create_dir_all(repo.join(d)).unwrap();
    }
    std::fs::write(
        repo.join("crates/ast/Cargo.toml"),
        "[package]\nname = \"ast\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(repo.join("crates/ast/src/lib.rs"), "// ast root\n").unwrap();
    std::fs::write(repo.join("crates/ast/src/dialect/mod.rs"), "// dialect\n").unwrap();
    std::fs::write(
        repo.join("crates/ast/src/precedence/mod.rs"),
        "// precedence\n",
    )
    .unwrap();
    std::fs::write(repo.join("crates/ast/src/vocab/mod.rs"), "// vocab\n").unwrap();
    std::fs::write(repo.join("crates/ast/src/nodes/mod.rs"), "// nodes\n").unwrap();
    std::fs::write(
        repo.join("crates/parser/Cargo.toml"),
        "[package]\nname = \"parser\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nast = { path = \"../ast\" }\n",
    )
    .unwrap();
    std::fs::write(repo.join("crates/parser/src/lib.rs"), "// parser\n").unwrap();
}

const AST_CONFIG: &str = "schema_version = 1\ntickets_dir = \"tickets\"\ndefault_base = \"main\"\n\
     [language]\nbackend = \"rust\"\n\
     [scopes]\n\
     \"ast-dialect-data\" = [\"crates/ast/src/dialect/**\", \"crates/ast/src/precedence/**\"]\n\
     \"ast-vocab\" = [\"crates/ast/src/vocab/**\", \"crates/ast/src/lib.rs\"]\n\
     \"ast-nodes\" = [\"crates/ast/src/nodes/**\"]\n\
     \"parser-scope\" = [\"crates/parser/**\"]\n\
     [scope_crates]\n\"ast-dialect-data\" = \"ast\"\n\"ast-vocab\" = \"ast\"\n\"ast-nodes\" = \"ast\"\n\"parser-scope\" = \"parser\"\n";

/// The reported bug: editing files inside the declared sub-crate scope of a
/// widely-depended-on crate must NOT be CONFLICT just because sibling scopes and
/// reverse-dependents map to the same crate — and a `paths` entry covers its file.
/// A genuine escape into a sibling sub-scope must still fire.
#[test]
fn guard_subcrate_scopes_do_not_trip_under_declaration() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_ast_workspace(repo);
    std::fs::write(repo.join("ticketsplease.toml"), AST_CONFIG).unwrap();
    // Declares only ast-dialect-data; lib.rs (which matches the ast-vocab glob) is
    // covered explicitly via --path.
    tkt(repo)
        .args([
            "create",
            "--id",
            "m1",
            "--title",
            "M1",
            "--scope",
            "ast-dialect-data",
            "--path",
            "crates/ast/src/lib.rs",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "m1", "--status", "in-progress"])
        .assert()
        .success();

    git_init_commit(repo);
    git(repo, &["checkout", "-q", "-b", "tkt/m1"]);
    // Edit only inside the declared area (dialect/, precedence/) + lib.rs (a --path).
    std::fs::write(repo.join("crates/ast/src/dialect/mod.rs"), "// edit\n").unwrap();
    std::fs::write(repo.join("crates/ast/src/precedence/mod.rs"), "// edit\n").unwrap();
    std::fs::write(repo.join("crates/ast/src/lib.rs"), "// edit root\n").unwrap();
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qam",
            "in-scope edit",
        ],
    );

    // Within its lane -> exit 0, no under-declaration, despite siblings/dependents
    // mapping to the same crate.
    let out = tkt(repo)
        .args(["guard", "tkt/m1", "--ticket", "m1", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["conflict"], false);
    assert_eq!(v["under_declared"].as_array().unwrap().len(), 0);
    assert_eq!(v["affected_causes"]["ast-dialect-data"], "direct");
    // An untouched sibling sub-scope sharing the crate is impact, not a direct touch.
    assert_eq!(v["affected_causes"]["ast-nodes"], "transitive");

    // Now genuinely escape into a sibling sub-scope (vocab/, undeclared, not a path).
    std::fs::write(repo.join("crates/ast/src/vocab/mod.rs"), "// escaped\n").unwrap();
    git(
        repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qam",
            "escape into vocab",
        ],
    );
    let out = tkt(repo)
        .args(["guard", "tkt/m1", "--ticket", "m1", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6), "a real escape must still fire");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let under: Vec<&str> = v["under_declared"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert_eq!(
        under,
        vec!["ast-vocab"],
        "only the escaped sub-scope, nothing else"
    );
}

/// `[language] reverse_dep_expansion = false` defaults the guard to path/crate-only
/// (as if --direct-only), so a transitive collision no longer fires.
#[test]
fn guard_config_disables_reverse_dep_expansion() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_cargo_fixture(repo);
    std::fs::write(
        repo.join("ticketsplease.toml"),
        "schema_version = 1\ntickets_dir = \"tickets\"\ndefault_base = \"main\"\n\
         [language]\nbackend = \"rust\"\nreverse_dep_expansion = false\n\
         [scopes]\n[scope_crates]\n\"a\" = \"crate-a\"\n\"b\" = \"crate-b\"\n",
    )
    .unwrap();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T", "--scope", "a"])
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

    // With expansion off, crate-b is never reached -> no transitive collision -> ok.
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t"])
        .assert()
        .success();
}

/// ux-sanitize-ticket-id + ux-trim-list-values + ux-status-parse-ergonomics.
#[test]
fn create_rejects_bad_ids_and_normalizes_input() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();

    // Path-traversal / non-slug ids are rejected at exit 3 (no file escapes the repo).
    tkt(repo)
        .args(["create", "--title", "x", "--id", "../../pwned"])
        .assert()
        .code(3);
    tkt(repo)
        .args(["create", "--title", "x", "--id", "Has Space"])
        .assert()
        .code(3);
    assert!(!std::path::Path::new("/tmp/pwned.md").exists());

    // Comma lists are trimmed + deduped; status parses case-insensitively.
    tkt(repo)
        .args([
            "create", "--id", "t", "--title", "T", "--scope", "a, b ,a", "--status", "TODO",
        ])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["show", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let scopes: Vec<&str> = v["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert_eq!(scopes, vec!["a", "b"]);
    assert_eq!(v["status"], "todo");
}

/// ux-no-config-error + ux-json-error-contract + ux-claim-done-exit-code.
#[test]
fn error_contract_and_exit_codes() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();

    // Before init: a friendly "not initialized" message, not a raw OS error.
    let out = tkt(repo).args(["list"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not initialized"));

    tkt(repo).args(["init", "--no-skill"]).assert().success();

    // Hard-fail under --format json: machine-readable envelope on stderr, clean stdout.
    let out = tkt(repo)
        .args(["show", "ghost", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(out.stdout.is_empty(), "stdout stays a clean result channel");
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "not-found");

    // Claiming a done ticket is a state conflict (exit 6), not invalid input (3).
    tkt(repo)
        .args(["create", "--id", "d", "--title", "D"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "d", "--status", "done"])
        .assert()
        .success();
    tkt(repo).args(["claim", "d", "--as", "w"]).assert().code(6);
}

/// ux-lint-cycle-exit-code: lint exits 5 on a cycle, matching ready/tracks. `link`
/// now rejects a cycle at write time, so the corrupt graph is created by hand-editing
/// the files (the state a careless manual edit would leave behind).
#[test]
fn lint_exits_5_on_cycle() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B"])
        .assert()
        .success();
    tkt(repo)
        .args(["link", "a", "--depends-on", "b"])
        .assert()
        .success();
    // Hand-edit b to depend on a, closing the cycle behind `link`'s guard.
    let bp = repo.join("tickets/b.md");
    let raw = std::fs::read_to_string(&bp).unwrap();
    std::fs::write(&bp, raw.replace("dependencies: []", "dependencies: [a]")).unwrap();
    tkt(repo).args(["lint"]).assert().code(5);
}

/// ux-why-exit-code: why exits 6 on conflict but still prints its report.
#[test]
fn why_exits_6_on_conflict() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A", "--scope", "core"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B", "--scope", "core"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["why", "a", "b", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["conflict"], true);
}

/// `list` filters compose, the empty result is a friendly message (not silence),
/// and an unparseable file degrades to a warning rather than failing the whole view.
#[test]
fn list_filters_empty_state_and_lenient() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "a",
            "--title",
            "A",
            "--scope",
            "core",
            "--tag",
            "x",
            "--priority",
            "p1",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "b",
            "--title",
            "B",
            "--scope",
            "io",
            "--tag",
            "y",
            "--priority",
            "p2",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "c",
            "--title",
            "C",
            "--scope",
            "core",
            "--priority",
            "p1",
        ])
        .assert()
        .success();

    let ids = |args: &[&str]| -> Vec<String> {
        let out = tkt(repo).args(args).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        v["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect()
    };

    assert_eq!(
        ids(&["list", "--scope", "core", "--format", "json"]),
        vec!["a", "c"]
    );
    assert_eq!(ids(&["list", "--tag", "x", "--format", "json"]), vec!["a"]);
    assert_eq!(
        ids(&["list", "--priority", "p1", "--format", "json"]),
        vec!["a", "c"]
    );
    // Filters compose (AND): core + p1 + tag x is just `a`.
    assert_eq!(
        ids(&[
            "list",
            "--scope",
            "core",
            "--priority",
            "p1",
            "--tag",
            "x",
            "--format",
            "json"
        ]),
        vec!["a"]
    );

    // Empty result set is a message, not blank output.
    let out = tkt(repo)
        .args(["list", "--status", "done"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success());
    assert!(text.contains("(no matching tickets)"), "got: {text:?}");

    // A corrupt ticket file degrades to a warning; the good tickets still list.
    std::fs::write(repo.join("tickets/bad.md"), "not valid frontmatter\n").unwrap();
    let out = tkt(repo)
        .args(["list", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "lenient list must not fail on a bad file"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let listed: Vec<&str> = v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(listed.contains(&"a") && listed.contains(&"b") && listed.contains(&"c"));
    assert!(
        !v["warnings"].as_array().unwrap().is_empty(),
        "the unparseable file should surface as a warning"
    );
}

/// Stage everything and commit with a fixed identity (test fixtures only).
fn git_commit_all(repo: &Path, msg: &str) {
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
            msg,
        ],
    );
}

/// Write the canonical `[scopes]` config (path-glob backend) into `repo`.
fn write_scope_config(repo: &Path, scopes: &str) {
    std::fs::write(
        repo.join("ticketsplease.toml"),
        format!(
            "schema_version = 1\ntickets_dir = \"tickets\"\ndefault_base = \"main\"\n\
             [language]\nbackend = \"none\"\n[scopes]\n{scopes}"
        ),
    )
    .unwrap();
}

/// guard collision detection sees a sibling's status on its own branch tip, not the
/// stale `todo` in the current checkout (the branch-per-ticket blind spot).
#[test]
fn guard_collision_fires_across_branch_tips() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"api\" = [\"src/api/**\"]\n");
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A", "--scope", "api"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B", "--scope", "api"])
        .assert()
        .success();
    std::fs::create_dir_all(repo.join("src/api")).unwrap();
    std::fs::write(repo.join("src/api/mod.rs"), "// base\n").unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git_commit_all(repo, "init");

    // b reaches `review` on its own branch — its open status lives there, not on main.
    git(repo, &["checkout", "-q", "-b", "tkt/b"]);
    tkt(repo)
        .args(["set", "b", "--status", "review"])
        .assert()
        .success();
    git_commit_all(repo, "b review");
    git(repo, &["checkout", "-q", "main"]);

    // a's branch edits the shared `api` scope.
    git(repo, &["checkout", "-q", "-b", "tkt/a"]);
    std::fs::write(repo.join("src/api/a.rs"), "// a\n").unwrap();
    tkt(repo)
        .args(["set", "a", "--status", "in-progress"])
        .assert()
        .success();
    git_commit_all(repo, "a work");

    // In this checkout b reads as todo, but its tip says review -> collision must fire.
    let out = tkt(repo)
        .args([
            "guard", "tkt/a", "--ticket", "a", "--base", "main", "--format", "json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(6),
        "cross-branch collision must fire"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["collisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["ticket"] == "b"),
        "should collide with b on `api`: {v}"
    );
}

/// guard reads the `[scopes]` contract from the base ref, so an emptied config on
/// the feature branch can't produce a false all-clear.
#[test]
fn guard_reads_config_from_base_not_branch() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"core\" = [\"core/**\"]\n\"io\" = [\"io/**\"]\n");
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T", "--scope", "core"])
        .assert()
        .success();
    std::fs::create_dir_all(repo.join("core")).unwrap();
    std::fs::create_dir_all(repo.join("io")).unwrap();
    std::fs::write(repo.join("core/a.txt"), "a\n").unwrap();
    std::fs::write(repo.join("io/b.txt"), "b\n").unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git_commit_all(repo, "init");

    // The branch drops the scope map entirely and edits io/ (undeclared by `t`).
    git(repo, &["checkout", "-q", "-b", "feat"]);
    write_scope_config(repo, "");
    std::fs::write(repo.join("io/b.txt"), "changed\n").unwrap();
    git_commit_all(repo, "empty config + edit io");

    // Branch config is empty (would be a no-op all-clear); base config still maps io,
    // so the under-declaration is caught.
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t", "--base", "main"])
        .assert()
        .code(6);
}

/// A changed file under no scope glob is invisible to collision detection — guard
/// surfaces the gap as a warning rather than staying silent.
#[test]
fn guard_warns_about_files_covered_by_no_scope() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"core\" = [\"core/**\"]\n");
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T", "--scope", "core"])
        .assert()
        .success();
    std::fs::create_dir_all(repo.join("misc")).unwrap();
    std::fs::write(repo.join("misc/util.txt"), "x\n").unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git_commit_all(repo, "init");
    git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("misc/util.txt"), "changed\n").unwrap();
    git_commit_all(repo, "edit unscoped file");

    let out = tkt(repo)
        .args([
            "guard", "feat", "--ticket", "t", "--base", "main", "--format", "json",
        ])
        .output()
        .unwrap();
    // No declared-scope escape (the file maps to no scope), so this is a clean exit
    // carrying a warning — not a failure.
    assert!(
        out.status.success(),
        "unscoped change is not itself a conflict"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("covered by no scope")),
        "should warn about the unscoped file: {v}"
    );
}

/// guard in a non-git directory fails with a clean message, not git's usage dump.
#[test]
fn guard_in_non_git_dir_errors_cleanly() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["guard", "feat", "--ticket", "t"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("requires a git repository"),
        "clean message expected, got: {err}"
    );
    assert!(
        !err.contains("usage: git"),
        "must not leak git usage: {err}"
    );
}

/// A bare `release` (no --as, no --force) must not silently drop a live claim.
#[test]
fn bare_release_does_not_drop_a_live_claim() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);
    tkt(repo)
        .args(["claim", "t", "--as", "alice"])
        .assert()
        .success();

    // Bare release is refused while someone holds it.
    tkt(repo).args(["release", "t"]).assert().code(6);
    let show = tkt(repo)
        .args(["show", "t", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(v["assignee"], "alice", "alice's claim must survive");

    // --force still overrides.
    tkt(repo)
        .args(["release", "t", "--force"])
        .assert()
        .success();
}

/// set resolves a ticket by filename but writes back to that same file, even when
/// the frontmatter id has drifted — no orphaned original, no duplicate id.
#[test]
fn set_writes_back_to_the_file_read_even_if_id_drifted() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "orig", "--title", "O"])
        .assert()
        .success();

    // Hand-edit the frontmatter id so it no longer matches the filename stem.
    let path = repo.join("tickets/orig.md");
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, raw.replace("id: orig", "id: drifted")).unwrap();

    // Operate by the filename id; the write must land back in orig.md.
    tkt(repo)
        .args(["set", "orig", "--status", "done"])
        .assert()
        .success();
    assert!(path.exists(), "the original file must be updated in place");
    assert!(
        !repo.join("tickets/drifted.md").exists(),
        "must not spawn a new file at the drifted id"
    );
    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("status: done"));
}

/// link rejects an edge that closes a multi-node cycle at write time (exit 5).
#[test]
fn link_rejects_a_multi_node_cycle() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    for id in ["a", "b", "c"] {
        tkt(repo)
            .args(["create", "--id", id, "--title", id])
            .assert()
            .success();
    }
    tkt(repo)
        .args(["link", "a", "--depends-on", "b"])
        .assert()
        .success();
    tkt(repo)
        .args(["link", "b", "--depends-on", "c"])
        .assert()
        .success();
    // c -> a closes a -> b -> c -> a.
    tkt(repo)
        .args(["link", "c", "--depends-on", "a"])
        .assert()
        .code(5);
    // The graph stayed acyclic, so lint is clean.
    tkt(repo).args(["lint"]).assert().success();
}

/// A dangling dependency can be removed after its target is deleted — `--remove`
/// never validates the target.
#[test]
fn link_remove_clears_a_dangling_dependency() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B"])
        .assert()
        .success();
    tkt(repo)
        .args(["link", "a", "--depends-on", "b"])
        .assert()
        .success();
    std::fs::remove_file(repo.join("tickets/b.md")).unwrap();
    // The reference is now dangling; lint flags it.
    tkt(repo).args(["lint"]).assert().failure();
    // Removal must succeed without the target existing.
    tkt(repo)
        .args(["link", "a", "--depends-on", "b", "--remove"])
        .assert()
        .success();
    tkt(repo).args(["lint"]).assert().success();
}

/// create and link treat a dangling dependency the same way: both permit it, and
/// lint reports it (one consistent model).
#[test]
fn create_and_link_treat_dangling_deps_consistently() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // create with a forward/dangling dep: permitted.
    tkt(repo)
        .args([
            "create",
            "--id",
            "a",
            "--title",
            "A",
            "--depends-on",
            "ghost",
        ])
        .assert()
        .success();
    // link with a dangling dep: also permitted (previously rejected with exit 4).
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B"])
        .assert()
        .success();
    tkt(repo)
        .args(["link", "b", "--depends-on", "ghost"])
        .assert()
        .success();
    // lint reports both dangling references.
    let out = tkt(repo)
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let missing: Vec<&str> = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "missing-dep")
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert!(
        missing.contains(&"a") && missing.contains(&"b"),
        "both dangling deps should lint: {v}"
    );
}

/// lint flags a ticket that declares a scope not defined in the config.
#[test]
fn lint_flags_unknown_scope_reference() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"core\" = [\"core/**\"]\n");
    // `cre` is a typo for `core`.
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T", "--scope", "cre"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "non-cycle lint findings are exit 3"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "unknown-scope" && d["id"] == "t"),
        "unknown scope should be flagged: {v}"
    );
}

/// `why x x` is a usage error, not a ticket trivially conflicting with itself.
#[test]
fn why_rejects_comparing_a_ticket_to_itself() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "x", "--title", "X", "--scope", "core"])
        .assert()
        .success();
    tkt(repo).args(["why", "x", "x"]).assert().code(3);
}

/// claim refuses a ticket whose dependencies aren't all done (matches ready/next).
#[test]
fn claim_refuses_unfinished_dependencies() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "base", "--title", "Base"])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "web",
            "--title",
            "Web",
            "--depends-on",
            "base",
        ])
        .assert()
        .success();
    git_init_commit(repo);
    // base is not done -> claiming web is refused (exit 6).
    tkt(repo)
        .args(["claim", "web", "--as", "w1"])
        .assert()
        .code(6);
    // Once base is done, web is claimable.
    tkt(repo)
        .args(["set", "base", "--status", "done"])
        .assert()
        .success();
    tkt(repo)
        .args(["claim", "web", "--as", "w1"])
        .assert()
        .success();
}

/// next --claim atomically claims the best free pick; --claim requires --as.
#[test]
fn next_claim_dispatches_atomically() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A", "--priority", "p0"])
        .assert()
        .success();
    git_init_commit(repo);
    // --claim without --as is a usage error.
    tkt(repo).args(["next", "--claim"]).assert().code(3);
    // --claim --as claims the top pick and reports it.
    let out = tkt(repo)
        .args(["next", "--claim", "--as", "w1", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["assignee"], "w1");
    assert_eq!(v["id"], "a");
    // a is now claimed (in-progress), so it's no longer ready.
    assert!(ready_ids(repo).is_empty());
}

/// `claims` lists holders + live/expired; `claim --force` steals a live lease.
#[test]
fn claims_view_and_force_steal() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);
    tkt(repo)
        .args(["claim", "t", "--as", "alice"])
        .assert()
        .success();

    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["claims", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let claims = v["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0]["assignee"], "alice");
    assert_eq!(claims[0]["live"], true);

    // A live lease can't be claimed by another without --force...
    tkt(repo)
        .args(["claim", "t", "--as", "bob"])
        .assert()
        .code(6);
    // ...but --force steals it.
    let out = tkt(repo)
        .args(["claim", "t", "--as", "bob", "--force", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["assignee"], "bob");
    assert_eq!(v["stolen"], true);
}

/// set --status done clears the claim (assignee + lease), so a done ticket is not
/// reported as owned.
#[test]
fn done_clears_the_claim() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);
    tkt(repo)
        .args(["claim", "t", "--as", "w1"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "t", "--status", "done"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["show", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(v["status"], "done");
    assert!(v["assignee"].is_null(), "done must clear assignee");
    assert!(v["lease_expires_at"].is_null(), "done must clear the lease");
}

/// Claiming a `todo` ticket then releasing it restores `todo`, not `ready`; the lease
/// is written as a bare integer in frontmatter (matching the JSON type).
#[test]
fn release_restores_status_and_lease_is_unquoted() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);
    tkt(repo)
        .args(["claim", "t", "--as", "w1"])
        .assert()
        .success();
    // While claimed, the lease is a bare integer (no surrounding quotes).
    let raw = std::fs::read_to_string(repo.join("tickets/t.md")).unwrap();
    let lease_line = raw
        .lines()
        .find(|l| l.starts_with("lease_expires_at:"))
        .unwrap();
    assert!(
        !lease_line.contains('"'),
        "lease must be unquoted: {lease_line:?}"
    );
    // Release restores the pre-claim status (todo), not ready.
    tkt(repo)
        .args(["release", "t", "--as", "w1"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["show", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        v["status"], "todo",
        "release should restore the original status"
    );
}

/// Re-claiming as the current holder is a renewal: no duplicate claim event.
#[test]
fn reclaim_renewal_emits_no_duplicate_event() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);
    tkt(repo)
        .args(["claim", "t", "--as", "w1"])
        .assert()
        .success();
    // Renew the same claim.
    let out = tkt(repo)
        .args(["claim", "t", "--as", "w1", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["renewed"], true);

    let events: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["events", "--ticket", "t", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let claim_events = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "claim")
        .count();
    assert_eq!(
        claim_events, 1,
        "a renewal must not add a second claim event"
    );
}

/// Batch create validates the whole batch before writing: a bad element aborts with
/// nothing written (no partial application).
#[test]
fn batch_create_is_atomic_on_a_bad_element() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    let specs = r#"[{"id":"good","title":"Good"},{"id":"bad","title":"Bad","status":"bogus"}]"#;
    let path = repo.join("b.json");
    std::fs::write(&path, specs).unwrap();
    tkt(repo)
        .args(["create", "--from", path.to_str().unwrap()])
        .assert()
        .code(3);
    assert!(
        !repo.join("tickets/good.md").exists(),
        "a bad element must abort the batch before writing the good one"
    );
}

/// Batch create with auto-ids is idempotent: re-running is unchanged, not a clone.
#[test]
fn batch_create_auto_ids_are_idempotent() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    let specs = r#"[{"title":"Auto One"}]"#;
    let path = repo.join("b.json");
    std::fs::write(&path, specs).unwrap();
    tkt(repo)
        .args(["create", "--from", path.to_str().unwrap()])
        .assert()
        .success();
    // Re-run: the same ticket is reported unchanged, and no `-2` duplicate appears.
    let out = tkt(repo)
        .args([
            "create",
            "--from",
            path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["created"], false, "re-run must be unchanged");
    assert!(repo.join("tickets/auto-one.md").exists());
    assert!(
        !repo.join("tickets/auto-one-2.md").exists(),
        "idempotent re-run must not clone the ticket"
    );
}

/// Batch JSON with an unknown key fails loudly instead of silently dropping it.
#[test]
fn batch_create_rejects_unknown_keys() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // `dependson` is a typo for `depends_on`.
    let specs = r#"[{"title":"X","dependson":["y"]}]"#;
    let path = repo.join("b.json");
    std::fs::write(&path, specs).unwrap();
    tkt(repo)
        .args(["create", "--from", path.to_str().unwrap()])
        .assert()
        .code(3);
}

/// Single and batch create share one result shape: a `results` array.
#[test]
fn create_emits_a_uniform_results_array() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    let out = tkt(repo)
        .args(["create", "--id", "t", "--title", "T", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "t");
    assert_eq!(results[0]["created"], true);
}

/// set can now edit title and paths, and add a dependency (with cycle rejection).
#[test]
fn set_edits_title_paths_and_dependencies() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B"])
        .assert()
        .success();

    tkt(repo)
        .args(["set", "a", "--title", "Renamed", "--add-path", "src/a/**"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(
        &tkt(repo)
            .args(["show", "a", "--format", "json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(v["title"], "Renamed");
    assert_eq!(v["paths"][0], "src/a/**");

    // set --add-dependency adds an edge...
    tkt(repo)
        .args(["set", "a", "--add-dependency", "b"])
        .assert()
        .success();
    // ...and rejects one that would close a cycle.
    tkt(repo)
        .args(["set", "b", "--add-dependency", "a"])
        .assert()
        .code(5);
}

/// delete removes a ticket file; list --hide-done filters completed tickets.
#[test]
fn delete_removes_a_ticket_and_hide_done_filters() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B"])
        .assert()
        .success();
    tkt(repo).args(["delete", "a"]).assert().success();
    assert!(!repo.join("tickets/a.md").exists());
    assert!(repo.join("tickets/b.md").exists());
    tkt(repo).args(["delete", "ghost"]).assert().code(4);

    tkt(repo)
        .args(["set", "b", "--status", "done"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["list", "--hide-done", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["tickets"].as_array().unwrap().is_empty(),
        "hide-done should drop the done ticket"
    );
}

/// rename moves the file, rewrites the id, and repoints dependents.
#[test]
fn rename_moves_file_and_repoints_dependents() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "old", "--title", "Old"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "dependent", "--title", "Dep"])
        .assert()
        .success();
    tkt(repo)
        .args(["link", "dependent", "--depends-on", "old"])
        .assert()
        .success();
    tkt(repo).args(["rename", "old", "new"]).assert().success();

    assert!(!repo.join("tickets/old.md").exists());
    assert!(repo.join("tickets/new.md").exists());
    let raw = std::fs::read_to_string(repo.join("tickets/new.md")).unwrap();
    assert!(raw.contains("id: new"));
    // The dependent was repointed, so lint sees no dangling reference.
    tkt(repo).args(["lint"]).assert().success();
    let dep = std::fs::read_to_string(repo.join("tickets/dependent.md")).unwrap();
    assert!(dep.contains("dependencies: [new]"));
}

/// doctor passes in a configured git repo and fails (cleanly) without git.
#[test]
fn doctor_checks_setup() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // No git yet -> the git_repo check fails.
    tkt(repo).args(["doctor"]).assert().failure();
    git_init_commit(repo);
    let out = tkt(repo)
        .args(["doctor", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "doctor should pass once git is set up"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
}

/// --dry-run previews create/set without writing.
#[test]
fn dry_run_previews_without_writing() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    let out = tkt(repo)
        .args([
            "create",
            "--id",
            "t",
            "--title",
            "T",
            "--dry-run",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dry_run"], true);
    assert!(
        !repo.join("tickets/t.md").exists(),
        "dry-run create must not write the file"
    );

    // Real create, then dry-run set leaves the ticket unchanged on disk.
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "t", "--status", "done", "--dry-run"])
        .assert()
        .success();
    let raw = std::fs::read_to_string(repo.join("tickets/t.md")).unwrap();
    assert!(raw.contains("status: todo"), "dry-run set must not persist");
}

/// tracks --parallel caps each batch to N tickets.
#[test]
fn tracks_parallel_caps_batch_size() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // Three disjoint (different scopes) ready tickets land in one conflict-free batch.
    for (id, scope) in [("a", "s1"), ("b", "s2"), ("c", "s3")] {
        tkt(repo)
            .args(["create", "--id", id, "--title", id, "--scope", scope])
            .assert()
            .success();
    }
    let out = tkt(repo)
        .args(["tracks", "--parallel", "2", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let batches = v["batches"].as_array().unwrap();
    assert!(
        batches.iter().all(|b| b.as_array().unwrap().len() <= 2),
        "each batch must be capped to 2: {v}"
    );
    let total: usize = batches.iter().map(|b| b.as_array().unwrap().len()).sum();
    assert_eq!(total, 3, "all tickets still appear");
}

/// Human event output carries a relative timestamp, not just a raw id.
#[test]
fn events_human_output_has_relative_time() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    git_init_commit(repo);
    tkt(repo)
        .args(["claim", "t", "--as", "w1"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["events", "--ticket", "t"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("ago") || text.contains("just now"),
        "events should show a relative time: {text:?}"
    );
}

/// A reply must target an existing comment; threads render nested in human output.
#[test]
fn comment_reply_to_is_validated_and_threaded() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    let out = tkt(repo)
        .args([
            "comment",
            "add",
            "t",
            "--body",
            "root comment",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let parent = serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // An unknown reply-to target is rejected.
    tkt(repo)
        .args(["comment", "add", "t", "--reply-to", "nope", "--body", "x"])
        .assert()
        .code(4);
    // A valid reply is accepted...
    tkt(repo)
        .args([
            "comment",
            "add",
            "t",
            "--reply-to",
            &parent,
            "--body",
            "a reply",
        ])
        .assert()
        .success();
    // ...and renders indented beneath its parent.
    let out = tkt(repo).args(["comment", "list", "t"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("root comment") && text.contains("a reply"));
    assert!(
        text.contains("    a reply"),
        "the reply should be indented under its parent: {text:?}"
    );
}

/// events validates its filters and signals a missing git repo (not silent empty).
#[test]
fn events_validates_filters_and_requires_git() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T"])
        .assert()
        .success();
    // No git yet -> events fails loudly instead of returning empty success.
    tkt(repo).args(["events"]).assert().failure();
    git_init_commit(repo);
    // Unknown event type is rejected.
    tkt(repo)
        .args(["events", "--type", "bogus"])
        .assert()
        .code(3);
    // Unknown ticket is rejected.
    tkt(repo)
        .args(["events", "--ticket", "ghost"])
        .assert()
        .code(4);
    // A valid (empty) query now succeeds.
    tkt(repo)
        .args(["events", "--ticket", "t"])
        .assert()
        .success();
}

/// init prints next-steps and warns when there's no git repo.
#[test]
fn init_prints_next_steps_and_warns_without_git() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    let out = tkt(repo).args(["init", "--no-skill"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Next steps"), "got: {text:?}");
    assert!(
        text.contains("not a git repository"),
        "should warn about missing git: {text:?}"
    );
}

/// `tkt guide` prints the conceptual model.
#[test]
fn guide_prints_the_concept_model() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    let out = tkt(repo).args(["guide"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Scopes") && text.contains("guard"));
}

/// --ignore-transitive still fails on a real (direct) under-declaration.
#[test]
fn guard_ignore_transitive_still_fails_on_direct() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"core\" = [\"core/**\"]\n\"io\" = [\"io/**\"]\n");
    tkt(repo)
        .args(["create", "--id", "t", "--title", "T", "--scope", "core"])
        .assert()
        .success();
    std::fs::create_dir_all(repo.join("core")).unwrap();
    std::fs::create_dir_all(repo.join("io")).unwrap();
    std::fs::write(repo.join("core/a.txt"), "a\n").unwrap();
    std::fs::write(repo.join("io/b.txt"), "b\n").unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git_commit_all(repo, "init");
    git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("io/b.txt"), "changed\n").unwrap();
    git_commit_all(repo, "edit io");
    // io is a direct under-declaration, so --ignore-transitive must still fail.
    tkt(repo)
        .args(["guard", "feat", "--ticket", "t", "--ignore-transitive"])
        .assert()
        .code(6);
}

/// reconcile flags the two drift directions (in-progress-no-branch, branch-not-started)
/// and orphan branches, and surfaces a worktree.
#[test]
fn reconcile_flags_board_git_drift() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "a", "--status", "in-progress"]) // in-progress, no branch -> stale-busy
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B"]) // todo, will get a branch -> stale-idle
        .assert()
        .success();
    git_init_commit(repo);
    git(repo, &["branch", "tkt/b"]);
    git(repo, &["branch", "tkt/ghost"]); // orphan branch, no ticket
    let wt = TempDir::new().unwrap();
    git(
        repo,
        &[
            "worktree",
            "add",
            "--quiet",
            wt.path().join("b").to_str().unwrap(),
            "tkt/b",
        ],
    );

    let out = tkt(repo)
        .args(["reconcile", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "drift should fail reconcile");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], false);
    let findings = v["findings"].as_array().unwrap();
    let by_id = |id: &str| findings.iter().find(|f| f["id"] == id).cloned();
    assert_eq!(by_id("a").unwrap()["issue"], "in-progress-no-branch");
    let b = by_id("b").unwrap();
    assert_eq!(b["issue"], "branch-without-active-ticket");
    assert_eq!(b["worktree"], true, "b's worktree should be detected");
    assert_eq!(by_id("ghost").unwrap()["issue"], "orphan-branch");
}

/// reconcile is clean (exit 0) when the board matches git.
#[test]
fn reconcile_clean_when_consistent() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args(["create", "--id", "x", "--title", "X"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "x", "--status", "in-progress"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "y", "--title", "Y"]) // todo, no branch -> consistent
        .assert()
        .success();
    git_init_commit(repo);
    git(repo, &["branch", "tkt/x"]); // x in-progress WITH a branch -> consistent
    tkt(repo).args(["reconcile"]).assert().success();
}

#[test]
fn related_links_are_non_blocking_and_queryable() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // `a` is created relating to `b`; `b` is created later and depends on a never-done
    // ticket, so it is NOT ready — but that must not hold `a` back.
    tkt(repo)
        .args(["create", "--id", "blocker", "--title", "Blocker"])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "a", "--title", "A", "--related", "b"])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "b",
            "--title",
            "B",
            "--depends-on",
            "blocker",
        ])
        .assert()
        .success();
    // `a` relates to `b` (not done) yet is still dispatchable.
    assert!(ready_ids(repo).contains(&"a".to_string()));

    // `link --related` and `set --add-related` both record without cycle-checking,
    // even when it forms a related cycle (b -> a while a -> b).
    tkt(repo)
        .args(["link", "b", "--related", "a"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "a", "--add-related", "blocker"])
        .assert()
        .success();

    // The field surfaces in JSON and is queryable; a related cycle lints clean.
    let out = tkt(repo)
        .args(["show", "a", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let related: Vec<&str> = v["related"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert!(related.contains(&"b") && related.contains(&"blocker"));
    tkt(repo).args(["lint"]).assert().success();

    // A self-relation is rejected, and a dangling related target is a lint finding
    // (exit 3) — but not a cycle.
    tkt(repo)
        .args(["set", "a", "--add-related", "a"])
        .assert()
        .code(3);
    tkt(repo)
        .args(["link", "a", "--related", "ghost"])
        .assert()
        .success();
    tkt(repo).args(["lint"]).assert().code(3);
}

#[test]
fn related_tickets_share_a_track_when_scopes_disjoint() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"core\" = [\"core/**\"]\n\"io\" = [\"io/**\"]\n");
    tkt(repo)
        .args([
            "create",
            "--id",
            "a",
            "--title",
            "A",
            "--scope",
            "core",
            "--related",
            "b",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B", "--scope", "io"])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["tracks", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // related (unlike a dependency) imposes no ordering: disjoint scopes -> one batch.
    assert_eq!(v["batches"].as_array().unwrap().len(), 1);
}

fn list_ids_where(repo: &Path, expr: &str) -> Vec<String> {
    let out = tkt(repo)
        .args(["list", "--where", expr, "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn list_where_filters_with_boolean_expressions() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "a",
            "--title",
            "A",
            "--priority",
            "p0",
            "--tag",
            "ux",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "b",
            "--title",
            "B",
            "--priority",
            "p1",
            "--tag",
            "ux",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "c",
            "--title",
            "C",
            "--priority",
            "p0",
            "--tag",
            "bug",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "b", "--status", "done"])
        .assert()
        .success();

    assert_eq!(
        list_ids_where(repo, "tag:ux AND NOT status:done"),
        vec!["a"]
    );
    assert_eq!(list_ids_where(repo, "priority:p0"), vec!["a", "c"]);
    assert_eq!(
        list_ids_where(repo, "(tag:bug OR tag:ux) AND priority:p0"),
        vec!["a", "c"]
    );
    assert!(list_ids_where(repo, "tag:missing").is_empty());

    // `--where` composes (AND) with the legacy single-axis flags.
    let out = tkt(repo)
        .args([
            "list",
            "--tag",
            "ux",
            "--where",
            "priority:p0",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["a"]);

    // A bad field or expression fails loudly (exit 3).
    tkt(repo)
        .args(["list", "--where", "bogus:x"])
        .assert()
        .code(3);
    tkt(repo)
        .args(["list", "--where", "(tag:ux"])
        .assert()
        .code(3);
}

#[test]
fn saved_views_round_trip_and_resolve_in_list() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "a",
            "--title",
            "A",
            "--tag",
            "epic",
            "--priority",
            "p0",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "b", "--title", "B", "--tag", "epic"])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "b", "--status", "done"])
        .assert()
        .success();

    // Save a view; the file lands under the committable .ticketsplease/ state dir.
    tkt(repo)
        .args(["view", "save", "open-epic", "tag:epic AND NOT status:done"])
        .assert()
        .success();
    assert!(repo.join(".ticketsplease/views.toml").exists());

    // A saved view resolves in `list --view` and composes (AND) with `--where`.
    let ids = |args: &[&str]| -> Vec<String> {
        let out = tkt(repo).args(args).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        v["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        ids(&["list", "--view", "open-epic", "--format", "json"]),
        vec!["a"]
    );
    assert_eq!(
        ids(&[
            "list",
            "--view",
            "open-epic",
            "--where",
            "priority:p0",
            "--format",
            "json"
        ]),
        vec!["a"]
    );

    // show / list / delete; saving a malformed expr fails; an unknown view is exit 4.
    let shown = tkt(repo)
        .args(["view", "show", "open-epic"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&shown.stdout).contains("tag:epic"));
    tkt(repo).args(["view", "list"]).assert().success();
    tkt(repo)
        .args(["view", "save", "bad", "bogus:x"])
        .assert()
        .code(3);
    tkt(repo).args(["list", "--view", "ghost"]).assert().code(4);
    tkt(repo)
        .args(["view", "delete", "open-epic"])
        .assert()
        .success();
    tkt(repo)
        .args(["view", "show", "open-epic"])
        .assert()
        .code(4);
    tkt(repo)
        .args(["view", "delete", "open-epic"])
        .assert()
        .code(4);
}

#[test]
fn bulk_set_where_edits_every_match() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    for id in ["a", "b", "c"] {
        tkt(repo)
            .args(["create", "--id", id, "--title", id, "--tag", "epic"])
            .assert()
            .success();
    }
    // `c` is not in the epic; only a and b should be touched.
    tkt(repo)
        .args(["set", "c", "--remove-tag", "epic"])
        .assert()
        .success();

    // Dry-run reports matches but writes nothing.
    let out = tkt(repo)
        .args([
            "set",
            "--where",
            "tag:epic",
            "--add-tag",
            "ready-soon",
            "--dry-run",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["matched"], 2);
    assert_eq!(v["dry_run"], true);
    let listed = list_ids_where(repo, "tag:ready-soon");
    assert!(listed.is_empty(), "dry-run wrote nothing: {listed:?}");

    // Real bulk edit: status + tag on every epic ticket.
    let out = tkt(repo)
        .args([
            "set",
            "--where",
            "tag:epic",
            "--status",
            "review",
            "--add-tag",
            "swept",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["matched"], 2);
    let changed = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["changed"] == true)
        .count();
    assert_eq!(changed, 2);
    let mut swept = list_ids_where(repo, "tag:swept AND status:review");
    swept.sort();
    assert_eq!(swept, vec!["a", "b"]);

    // Bulk rejects single-target-only edits.
    tkt(repo)
        .args(["set", "--where", "tag:epic", "--title", "nope"])
        .assert()
        .code(3);
    tkt(repo)
        .args(["set", "--where", "tag:epic", "--body", "nope"])
        .assert()
        .code(3);
    // id and --where are mutually exclusive; neither is an error too.
    tkt(repo)
        .args(["set", "a", "--where", "tag:epic", "--add-tag", "x"])
        .assert()
        .code(3);
    tkt(repo).args(["set", "--add-tag", "x"]).assert().code(3);
}

#[test]
fn create_from_toml_manifest_and_stdin_sniff() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();

    // A TOML manifest with [[ticket]] tables, including related + a dependency.
    let manifest = repo.join("backlog.toml");
    std::fs::write(
        &manifest,
        "[[ticket]]\nid = \"base\"\ntitle = \"Base\"\n\n\
         [[ticket]]\nid = \"feat\"\ntitle = \"Feature\"\ndepends_on = [\"base\"]\nrelated = [\"base\"]\ntags = [\"epic\"]\n",
    )
    .unwrap();
    tkt(repo)
        .args(["create", "--from", manifest.to_str().unwrap()])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["show", "feat", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dependencies"][0], "base");
    assert_eq!(v["related"][0], "base");

    // stdin starting with `[[` is sniffed as TOML even without an extension.
    tkt(repo)
        .args(["create", "--from", "-"])
        .write_stdin("[[ticket]]\nid = \"from-stdin\"\ntitle = \"Stdin TOML\"\n")
        .assert()
        .success();
    tkt(repo).args(["show", "from-stdin"]).assert().success();

    // A bad TOML manifest fails loudly (exit 3).
    let bad = repo.join("bad.toml");
    std::fs::write(&bad, "[[ticket]]\nid = \"x\"\n").unwrap(); // missing required title
    tkt(repo)
        .args(["create", "--from", bad.to_str().unwrap()])
        .assert()
        .code(3);
}

#[test]
fn rollup_reports_counts_frontier_and_blocked() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // An initiative tagged `m1`: base (done), ready (todo, deps satisfied),
    // blocked (todo, waiting on an unfinished dep), plus an untagged ticket.
    tkt(repo)
        .args([
            "create",
            "--id",
            "base",
            "--title",
            "Base",
            "--tag",
            "m1",
            "--priority",
            "p0",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["set", "base", "--status", "done"])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "frontier",
            "--title",
            "Frontier",
            "--tag",
            "m1",
            "--depends-on",
            "base",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "gate", "--title", "Gate", "--tag", "m1"])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "blocked",
            "--title",
            "Blocked",
            "--tag",
            "m1",
            "--depends-on",
            "gate",
        ])
        .assert()
        .success();
    tkt(repo)
        .args(["create", "--id", "other", "--title", "Other"])
        .assert()
        .success();

    let out = tkt(repo)
        .args(["rollup", "--tag", "m1", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["total"], 4, "the untagged ticket is excluded");
    assert_eq!(v["done"], 1);
    assert_eq!(v["percent_done"], 25);
    assert_eq!(v["by_status"]["done"], 1);
    assert_eq!(v["by_status"]["todo"], 3);

    // Frontier = dispatchable within the initiative (base done -> frontier & gate ready).
    let mut ready: Vec<&str> = v["ready"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    ready.sort();
    assert_eq!(ready, vec!["frontier", "gate"]);

    // `blocked` waits on a not-done dep, and lists the unmet dependency.
    let blocked = v["blocked"].as_array().unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["id"], "blocked");
    assert_eq!(blocked[0]["unmet"][0], "gate");

    // No selector rolls up the whole board (includes the untagged ticket).
    let out = tkt(repo)
        .args(["rollup", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["total"], 5);
}

#[test]
fn graph_and_path_export_the_dependency_dag() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // base <- mid <- leaf (deps), plus a non-blocking related edge leaf ~ base.
    tkt(repo)
        .args(["create", "--id", "base", "--title", "Base", "--tag", "m1"])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "mid",
            "--title",
            "Mid",
            "--tag",
            "m1",
            "--depends-on",
            "base",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "leaf",
            "--title",
            "Leaf",
            "--tag",
            "m1",
            "--depends-on",
            "mid",
            "--related",
            "base",
        ])
        .assert()
        .success();
    // An unrelated, untagged ticket that the tag filter should exclude.
    tkt(repo)
        .args(["create", "--id", "other", "--title", "Other"])
        .assert()
        .success();

    // JSON graph, restricted to the m1 initiative.
    let out = tkt(repo)
        .args(["graph", "--tag", "m1", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["nodes"].as_array().unwrap().len(), 3, "other is excluded");
    let edges = v["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2);
    assert!(edges
        .iter()
        .any(|e| e["from"] == "mid" && e["to"] == "base"));
    let related = v["related_edges"].as_array().unwrap();
    assert_eq!(related.len(), 1);
    assert_eq!(related[0]["from"], "leaf");
    assert_eq!(related[0]["to"], "base");
    let base = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "base")
        .unwrap();
    assert_eq!(base["downstream_count"], 2);

    // DOT output is a digraph with solid dep edges and a dashed related edge.
    let dot = tkt(repo)
        .args(["graph", "--tag", "m1", "--dot"])
        .output()
        .unwrap();
    let dot = String::from_utf8_lossy(&dot.stdout);
    assert!(dot.contains("digraph tickets {"));
    assert!(dot.contains("\"mid\" -> \"base\";"));
    assert!(dot.contains("style=dashed"));

    // path: the critical prerequisite chain to leaf, root-first.
    let out = tkt(repo)
        .args(["path", "leaf", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["length"], 3);
    let ids: Vec<&str> = v["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["base", "mid", "leaf"]);
    tkt(repo).args(["path", "ghost"]).assert().code(4);
}

fn body_of(repo: &Path, id: &str) -> String {
    std::fs::read_to_string(repo.join("tickets").join(format!("{id}.md"))).unwrap()
}

#[test]
fn create_template_scaffolds_the_body() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    // init seeds the example templates under the committable state dir.
    assert!(repo.join(".ticketsplease/templates/default.md").exists());
    assert!(repo.join(".ticketsplease/templates/audit.md").exists());

    // --template fills the body and substitutes {{title}}/{{id}}.
    tkt(repo)
        .args([
            "create",
            "--id",
            "scaffolded",
            "--title",
            "Scaffolded Thing",
            "--template",
            "audit",
        ])
        .assert()
        .success();
    let body = body_of(repo, "scaffolded");
    assert!(
        body.contains("# Scaffolded Thing"),
        "title substituted: {body}"
    );
    assert!(body.contains("## Gap"), "audit scaffold used");
    assert!(body.contains("scaffolded"), "id substituted");
    assert!(
        !body.contains("{{title}}") && !body.contains("{{id}}"),
        "no placeholders left"
    );

    // An explicit --body overrides the template.
    tkt(repo)
        .args([
            "create",
            "--id",
            "explicit",
            "--title",
            "Explicit",
            "--template",
            "audit",
            "--body",
            "just this",
        ])
        .assert()
        .success();
    let body = body_of(repo, "explicit");
    assert!(body.contains("just this") && !body.contains("## Gap"));

    // An unknown template is exit 4.
    tkt(repo)
        .args(["create", "--id", "x", "--title", "X", "--template", "ghost"])
        .assert()
        .code(4);

    // A batch spec can name a template too (JSON), with per-id substitution.
    tkt(repo)
        .args(["create", "--from", "-"])
        .write_stdin(r#"[{"id":"batched","title":"Batched","template":"default"}]"#)
        .assert()
        .success();
    let body = body_of(repo, "batched");
    assert!(body.contains("# Batched") && body.contains("## Goal"));
}

#[test]
fn shared_scopes_co_schedule_and_are_validated() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"core\" = [\"core/**\"]\n\"io\" = [\"io/**\"]\n");

    // Two tickets that both claim `core` additively are compatible -> one track.
    tkt(repo)
        .args([
            "create",
            "--id",
            "a",
            "--title",
            "A",
            "--shared-scope",
            "core",
        ])
        .assert()
        .success();
    tkt(repo)
        .args([
            "create",
            "--id",
            "b",
            "--title",
            "B",
            "--shared-scope",
            "core",
        ])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["tracks", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["batches"].as_array().unwrap().len(),
        1,
        "additive core co-schedules"
    );
    // `why` agrees they don't conflict (exit 0).
    tkt(repo).args(["why", "a", "b"]).assert().success();

    // A ticket that rewrites `core` (exclusive) conflicts with the additive ones.
    tkt(repo)
        .args(["create", "--id", "c", "--title", "C", "--scope", "core"])
        .assert()
        .success();
    tkt(repo).args(["why", "a", "c"]).assert().code(6);

    // The field surfaces in JSON, and set --add-shared-scope edits it.
    let out = tkt(repo)
        .args(["show", "a", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["shared_scopes"][0], "core");
    tkt(repo)
        .args(["set", "b", "--add-shared-scope", "io"])
        .assert()
        .success();

    // A scope claimed both exclusive and shared on one ticket is a lint finding.
    tkt(repo)
        .args([
            "create",
            "--id",
            "x",
            "--title",
            "X",
            "--scope",
            "core",
            "--shared-scope",
            "core",
        ])
        .assert()
        .success();
    let out = tkt(repo)
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "scope-mode-conflict" && d["id"] == "x"));
    tkt(repo).args(["lint"]).assert().code(3);
}

#[test]
fn max_overlap_fills_workers_and_reports_cost() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    tkt(repo).args(["init", "--no-skill"]).assert().success();
    write_scope_config(repo, "\"core\" = [\"core/**\"]\n");
    // Three tickets that all rewrite `core` (exclusive): disjoint width 1.
    for id in ["a", "b", "c"] {
        tkt(repo)
            .args(["create", "--id", id, "--title", id, "--scope", "core"])
            .assert()
            .success();
    }

    let run = |args: &[&str]| -> serde_json::Value {
        let out = tkt(repo).args(args).output().unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };

    // Strict (default budget 0): only one of the three fits.
    let v = run(&["next", "--parallel", "3", "--format", "json"]);
    assert_eq!(v["picks"].as_array().unwrap().len(), 1);

    // Budget 1: all three fill, every pair costs 1, set overlap_cost = 3.
    let v = run(&[
        "next",
        "--parallel",
        "3",
        "--max-overlap",
        "1",
        "--format",
        "json",
    ]);
    assert_eq!(v["picks"].as_array().unwrap().len(), 3);
    assert_eq!(v["overlap_cost"], 3);
    assert!(v["picks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["conflicts_with"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["cost"] == 1)));

    // `--allow-overlap` is the unbounded alias.
    let v = run(&[
        "next",
        "--parallel",
        "3",
        "--allow-overlap",
        "--format",
        "json",
    ]);
    assert_eq!(v["picks"].as_array().unwrap().len(), 3);

    // tracks: strict = 3 batches; budget 1 = one batch with tolerated cost 3.
    let v = run(&["tracks", "--format", "json"]);
    assert_eq!(v["batches"].as_array().unwrap().len(), 3);
    let v = run(&["tracks", "--max-overlap", "1", "--format", "json"]);
    assert_eq!(v["batches"].as_array().unwrap().len(), 1);
    assert_eq!(v["overlap_cost"], 3);

    // A malformed budget fails loudly.
    tkt(repo)
        .args(["next", "--max-overlap", "lots"])
        .assert()
        .code(3);
}
