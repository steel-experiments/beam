// ABOUTME: Round-trip tests: snapshot a repo, restore it with the sandbox scripts, change it, bring it down.
// ABOUTME: The "sandbox" is a second local directory, so these tests need only git and sh.

use crate::down::merge_files;
use crate::git::{self, BackOutcome, Snap};
use crate::pack::{self, DiskEntry};
use crate::remote;
use crate::util::run;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

fn sh(dir: &Path, script: &str) -> String {
    run(Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1"))
    .unwrap_or_else(|e| panic!("{script}: {e:#}"))
}

const GIT: &str = "git -c user.name=t -c user.email=t@t -c commit.gpgsign=false";

/// A repo with each kind of worktree state.
fn make_repo(dir: &Path) {
    sh(
        dir,
        &format!(
            "git init -q -b main . && printf 'ignored.log\\n' > .gitignore \
         && echo a > a.txt && echo b > b.txt && echo c > c.txt && echo d > d.txt \
         && printf '#!/bin/sh\\n' > run.sh && chmod 755 run.sh && ln -s a.txt link \
         && git add -A && {GIT} commit -q -m one \
         && echo a2 > a.txt && git add a.txt && echo a3 > a.txt \
         && echo s > staged.txt && git add staged.txt \
         && echo b2 > b.txt && rm c.txt && git rm -q d.txt \
         && echo u > u.txt && mkdir -p sub && echo u2 > sub/u2.txt \
         && echo junk > ignored.log && echo SECRET=1 > .env"
        ),
    );
}

fn state_of(dir: &Path) -> String {
    sh(
        dir,
        "git status --porcelain=v1 -uall && git diff && git diff --cached && git log --format=%s \
             && cat a.txt b.txt staged.txt u.txt sub/u2.txt && git ls-files --stage run.sh | cut -d\" \" -f1 && readlink link",
    )
}

/// Pack like "beam up" does, restore like the sandbox does.
fn beam_up(src: &Path, dst: &Path, stage: &Path) -> Snap {
    let bundle = stage.join("repo.bundle");
    std::fs::create_dir_all(stage).unwrap();
    let sent = git::snapshot(src, "refs/beam/t/up", Some(&bundle), None).unwrap();
    std::fs::write(stage.join("snapshot.sh"), git::SNAPSHOT_SH).unwrap();
    std::fs::create_dir_all(stage.join("extras")).unwrap();
    std::fs::copy(src.join(".env"), stage.join("extras/.env")).unwrap();
    let s = |p: &Path| p.to_string_lossy().to_string();
    let script = remote::restore(&remote::RestoreVars {
        stage: &s(stage),
        project: &s(dst),
        home: &s(&stage.with_file_name("rhome")),
        refname: "refs/beam/t/up",
        branch: &sent.branch,
        head: &sent.head,
        idx_tree: &sent.idx_tree,
        wt_tree: &sent.wt_tree,
        origin: "",
        git_name: "t",
        git_email: "t@t",
    });
    sh(stage, &script);
    sent
}

/// Pack like the sandbox does for "beam down". Returns the archive entries.
fn pack_down(
    dst: &Path,
    stage: &Path,
    home: &Path,
    sent: &Snap,
    agent_paths: &[String],
) -> Vec<DiskEntry> {
    let s = |p: &Path| p.to_string_lossy().to_string();
    sh(
        stage,
        &remote::pack_back(
            &s(stage),
            &s(dst),
            &s(home),
            "refs/beam/t/back",
            &sent.wt_commit,
            agent_paths,
            &[],
        ),
    );
    pack::extract(
        &stage.join("back.tar.gz"),
        &stage.join("incoming"),
        agent_paths,
    )
    .unwrap()
}

fn apply(src: &Path, stage: &Path, entries: &[DiskEntry], sent: &Snap) -> BackOutcome {
    let info = entries.iter().find(|e| e.path == "info").unwrap();
    let remote = Snap::from_kv(&std::fs::read_to_string(&info.file).unwrap()).unwrap();
    let b = entries.iter().find(|e| e.path == "repo.bundle").unwrap();
    let bp = stage.join("down.bundle");
    std::fs::copy(&b.file, &bp).unwrap();
    git::fetch_bundle(src, &bp, "refs/beam/t/back").unwrap();
    git::apply_back(src, sent, &remote, "refs/beam/t/check").unwrap()
}

#[test]
fn worktree_state_survives_up_and_down() {
    let t = tempfile::tempdir().unwrap();
    let (src, dst, stage) = (
        t.path().join("src"),
        t.path().join("dst"),
        t.path().join("stage"),
    );
    std::fs::create_dir_all(&src).unwrap();
    make_repo(&src);
    let before = state_of(&src);

    let sent = beam_up(&src, &dst, &stage);
    assert_eq!(
        state_of(&dst),
        before,
        "the sandbox must have the same git state"
    );
    assert!(
        !dst.join("ignored.log").exists(),
        "ignored files stay local"
    );
    assert_eq!(
        std::fs::read_to_string(dst.join(".env")).unwrap(),
        "SECRET=1\n"
    );
    assert!(
        git::snapshot(&dst, "refs/beam/t/x", None, None)
            .unwrap()
            .same_state(&sent)
    );

    // The agent works in the sandbox.
    sh(
        &dst,
        &format!(
            "{GIT} commit -q -m two && echo a4 > a.txt && rm u.txt && echo n > new.txt \
         && git checkout -q -b feature && echo b3 > b.txt && git add b.txt"
        ),
    );
    let home = t.path().join("rhome");
    std::fs::create_dir_all(home.join(".claude/projects/p")).unwrap();
    std::fs::write(home.join(".claude/projects/p/s.jsonl"), "one\ntwo\n").unwrap();
    let remote_state = sh(
        &dst,
        "git status --porcelain=v1 -uall && git diff && git diff --cached \
                                 && git log --format=%s && git branch --show-current && cat a.txt new.txt",
    );

    let entries = pack_down(
        &dst,
        &stage,
        &home,
        &sent,
        &[".claude/projects/p".into(), ".claude/nothing".into()],
    );
    assert_eq!(apply(&src, &stage, &entries, &sent), BackOutcome::Applied);
    let local_state = sh(
        &src,
        "git status --porcelain=v1 -uall && git diff && git diff --cached \
                                && git log --format=%s && git branch --show-current && cat a.txt new.txt",
    );
    assert_eq!(
        local_state, remote_state,
        "local must match the sandbox after beam down"
    );
    assert!(src.join("ignored.log").exists(), "local ignored files stay");
    assert!(
        !src.join("u.txt").exists(),
        "a file deleted in the sandbox is deleted locally"
    );

    let agent: Vec<&DiskEntry> = entries
        .iter()
        .filter(|e| e.path.starts_with(".claude/"))
        .collect();
    let names: Vec<&str> = agent.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, vec![".claude/projects/p/s.jsonl"]);
    let lhome = t.path().join("lhome");
    let r = merge_files(
        &lhome,
        &BTreeMap::new(),
        &agent,
        "",
        &[],
        &t.path().join("conflicts"),
    )
    .unwrap();
    assert!(r.conflicts.is_empty());
    assert_eq!(
        std::fs::read(lhome.join(".claude/projects/p/s.jsonl")).unwrap(),
        std::fs::read(&agent[0].file).unwrap()
    );
}

#[test]
fn local_changes_keep_remote_work_aside() {
    let t = tempfile::tempdir().unwrap();
    let (src, dst, stage) = (
        t.path().join("src"),
        t.path().join("dst"),
        t.path().join("stage"),
    );
    std::fs::create_dir_all(&src).unwrap();
    make_repo(&src);
    let sent = beam_up(&src, &dst, &stage);
    sh(
        &dst,
        &format!("{GIT} commit -q -m remote-commit && echo remote > a.txt"),
    );
    std::fs::write(src.join("b.txt"), "local edit while away\n").unwrap();
    let local_before = state_of(&src);

    let home = t.path().join("rhome");
    std::fs::create_dir_all(&home).unwrap();
    let entries = pack_down(&dst, &stage, &home, &sent, &[]);
    let out = apply(&src, &stage, &entries, &sent);
    assert!(matches!(out, BackOutcome::KeptAside), "{out:?}");
    assert_eq!(state_of(&src), local_before, "local work must not change");
    let remote = Snap::from_kv(
        &std::fs::read_to_string(&entries.iter().find(|e| e.path == "info").unwrap().file).unwrap(),
    )
    .unwrap();
    let recovery = t.path().join("recovery");
    git::recovery_worktree(&src, &recovery, &remote).unwrap();
    assert_eq!(sh(&recovery, "git log -1 --format=%s"), "remote-commit");
    assert_eq!(sh(&recovery, "cat a.txt"), "remote");
}

#[test]
fn empty_commit_without_an_index_can_be_snapshotted() {
    let d = tempfile::tempdir().unwrap();
    sh(
        d.path(),
        &format!("git init -q -b main . && {GIT} commit -q --allow-empty -m empty"),
    );
    let index = d.path().join(".git/index");
    if index.exists() {
        std::fs::remove_file(index).unwrap();
    }
    let sent = git::snapshot(d.path(), "refs/beam/empty/up", None, None).unwrap();
    assert_eq!(sent.idx_tree, sent.wt_tree);
}

#[test]
fn returning_tracked_file_never_overwrites_a_local_ignored_file() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    let dst = d.path().join("dst");
    let stage = d.path().join("stage");
    std::fs::create_dir(&src).unwrap();
    make_repo(&src);
    let sent = beam_up(&src, &dst, &stage);
    sh(&dst, "echo remote > ignored.log && git add -f ignored.log");
    let entries = pack_down(&dst, &stage, &stage.with_file_name("rhome"), &sent, &[]);
    let outcome = apply(&src, &stage, &entries, &sent);
    assert_eq!(outcome, BackOutcome::KeptAside);
    assert_eq!(
        std::fs::read_to_string(src.join("ignored.log")).unwrap(),
        "junk\n"
    );
}

#[test]
fn another_adapter_discovers_launches_observes_and_returns_its_own_files() {
    use crate::agent::{
        Adapter,
        evidence::{Source, Task},
        testing::Fixture,
    };
    let adapter = Fixture;
    let t = tempfile::tempdir().unwrap();
    let (src, dst, stage) = (
        t.path().join("src"),
        t.path().join("dst"),
        t.path().join("stage"),
    );
    let (local_home, remote_home) = (t.path().join("lhome"), t.path().join("rhome"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(local_home.join(".fixture/conversations")).unwrap();
    std::fs::write(
        local_home.join(".fixture/conversations/session.txt"),
        "original conversation\n",
    )
    .unwrap();
    make_repo(&src);
    let session = adapter.find_session(&local_home, &src, "session").unwrap();
    let paths = adapter.session_paths(&local_home, &session);
    let hashes = crate::plan::hashes(&local_home, &paths).unwrap();
    for path in &paths {
        let dest = stage.join("home").join(path);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(local_home.join(path), dest).unwrap();
    }
    for (path, bytes) in adapter.defaults(&local_home, &src).unwrap().files {
        let dest = stage.join("defaults").join(path);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(dest, bytes).unwrap();
    }
    let sent = beam_up(&src, &dst, &stage);
    assert!(remote_home.join(".fixture/config").is_file());
    sh(
        &stage,
        &format!(
            "S={}\nH={}\n{}\nresume 'continued conversation'",
            crate::util::sh_quote(&stage.to_string_lossy()),
            crate::util::sh_quote(&remote_home.to_string_lossy()),
            adapter.resume_fn(&session.id)
        ),
    );
    let snapshot = crate::monitor::observe(
        &adapter,
        "running".into(),
        vec![],
        |script| Ok(sh(&stage, script)),
        &stage.to_string_lossy(),
        "test",
    );
    assert_eq!(snapshot.task.state, Task::CompletionReported);
    assert_eq!(snapshot.task.evidence.source, Source::ClientEvent);
    let entries = pack_down(
        &dst,
        &stage,
        &remote_home,
        &sent,
        &adapter.return_paths(&src),
    );
    let files: Vec<_> = entries
        .iter()
        .filter(|e| crate::agent::contains_path(&adapter.return_paths(&src), &e.path))
        .collect();
    let merged = merge_files(
        &local_home,
        &hashes,
        &files,
        "",
        &[],
        &t.path().join("conflicts"),
    )
    .unwrap();
    assert_eq!(merged.conflicts.len(), 0);
    assert_eq!(
        std::fs::read_to_string(local_home.join(&paths[0])).unwrap(),
        "original conversation\ncontinued conversation\n"
    );
    assert!(!local_home.join(".fixture/config").exists());
}

/// A private tmux server for one test. It stops on drop, also when an assertion fails.
struct Tmux(std::path::PathBuf);
impl Tmux {
    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env("TMUX_TMPDIR", &self.0)
            .env_remove("TMUX")
            .env_remove("BEAM_STAGE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c
    }
    fn alive(&self, session: &str) -> bool {
        self.cmd("tmux")
            .args(["has-session", "-t", &format!("={session}")])
            .output()
            .unwrap()
            .status
            .success()
    }
}
impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = self.cmd("tmux").arg("kill-server").output();
    }
}

/// A sandbox with one transfer: the restored project, its return script, the sandbox beam
/// command, and an agent in tmux that records its exit on Ctrl-C like the launcher does.
struct SandboxReturn {
    t: tempfile::TempDir,
    src: std::path::PathBuf,
    dst: std::path::PathBuf,
    stage: std::path::PathBuf,
    root: std::path::PathBuf,
    sent: Snap,
    tmux: Tmux,
}
impl SandboxReturn {
    const ID: &str = "77-1";
    fn new(agent: &str) -> SandboxReturn {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join(".beam");
        let (src, dst, stage) = (
            t.path().join("src"),
            t.path().join("dst"),
            root.join("remote").join(Self::ID),
        );
        std::fs::create_dir_all(&src).unwrap();
        make_repo(&src);
        let sent = beam_up(&src, &dst, &stage);
        let s = |p: &Path| p.to_string_lossy().to_string();
        let home = t.path().join("rhome");
        std::fs::create_dir_all(&home).unwrap();
        let pack = remote::pack_back(
            &s(&stage),
            &s(&dst),
            &s(&home),
            "refs/beam/t/back",
            &sent.wt_commit,
            &[],
            &[],
        );
        let name = format!("beam-{}", Self::ID);
        std::fs::write(
            stage.join("return.sh"),
            remote::return_script(&s(&stage), &name, true, &pack),
        )
        .unwrap();
        std::fs::write(stage.join("project"), s(&src)).unwrap();
        sh(t.path(), &remote::install_cli(&s(&root), false));
        let tmux = Tmux(t.path().join("tmux"));
        std::fs::create_dir_all(&tmux.0).unwrap();
        // Like the launcher, put the sandbox beam command first on PATH.
        let program = format!(
            "PATH={}:$PATH; trap 'echo 0 > {}; exit 0' INT; {agent} while :; do sleep 0.1; done",
            crate::util::sh_quote(&s(&root.join("bin"))),
            crate::util::sh_quote(&s(&stage.join("agent.exit")))
        );
        let started = tmux
            .cmd("tmux")
            .args(["new-session", "-d", "-s", &name, "-c", &s(&dst)])
            .arg(format!("sh -c {}", crate::util::sh_quote(&program)))
            .env("BEAM_STAGE", &stage)
            .output()
            .unwrap();
        assert!(started.status.success(), "{started:?}");
        SandboxReturn {
            t,
            src,
            dst,
            stage,
            root,
            sent,
            tmux,
        }
    }
    fn beam(&self, args: &[&str]) -> std::process::Output {
        self.tmux
            .cmd(&self.root.join("bin/beam").to_string_lossy())
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }
    fn events(&self) -> String {
        std::fs::read_to_string(self.stage.join("events.tsv")).unwrap_or_default()
    }
    /// The local half of the return: the local pack reuses the sandbox package, then applies it.
    fn bring_home(&self) {
        let home = self.t.path().join("rhome");
        let entries = pack_down(&self.dst, &self.stage, &home, &self.sent, &[]);
        let out = apply(&self.src, &self.stage, &entries, &self.sent);
        assert!(matches!(out, BackOutcome::Applied), "{out:?}");
        assert_eq!(sh(&self.src, "git log -1 --format=%s"), "remote-commit");
    }
}

#[test]
fn beam_down_in_a_sandbox_shell_stops_the_agent_and_packs_for_the_local_return() {
    let sb = SandboxReturn::new("");
    sh(&sb.dst, &format!("{GIT} commit -q -m remote-commit"));
    let out = sb.beam(&["down"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("run: beam down"), "{stdout}");
    assert!(stdout.contains("do not return"), "{stdout}");
    assert!(sb.stage.join("return-ready").exists());
    assert!(sb.stage.join("back.tar.gz").exists());
    assert!(!sb.stage.join("pack-lock").exists());
    assert!(
        sb.stage.join("agent.exit").exists(),
        "the agent did not get Ctrl-C"
    );
    assert!(!sb.tmux.alive(&format!("beam-{}", SandboxReturn::ID)));
    let events = sb.events();
    assert!(events.contains("\treturn-requested\tbeam down in the sandbox"));
    assert!(events.contains("\treturn-ready\t"));

    let again = sb.beam(&["down"]);
    assert!(again.status.success(), "{again:?}");
    assert!(String::from_utf8_lossy(&again.stdout).contains("already packed"));
    let attach = sb.beam(&["attach"]);
    assert!(!attach.status.success());
    assert!(String::from_utf8_lossy(&attach.stderr).contains("packed for the return"));
    let ls = sb.beam(&["ls"]);
    assert!(String::from_utf8_lossy(&ls.stdout).contains("packed; run beam down locally"));
    let status = sb.beam(&["status"]);
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("runs on your local machine"));

    sb.bring_home();
}

#[test]
fn beam_down_from_the_agent_terminal_lets_the_agent_reply_before_it_stops() {
    let sb = SandboxReturn::new(&format!(
        "{GIT} commit -q --allow-empty -m remote-commit; beam down > beam-down.out 2>&1; touch replied;"
    ));
    let replied = sb.dst.join("replied");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !sb.stage.join("return-ready").exists() && !sb.stage.join("return-failed").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the pack did not finish: {} {:?}",
            sb.events(),
            std::fs::read_to_string(sb.dst.join("beam-down.out"))
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(replied.exists(), "beam down did not return to the agent");
    let said = std::fs::read_to_string(sb.dst.join("beam-down.out")).unwrap();
    assert!(said.contains("In 5 seconds the agent stops"), "{said}");
    let events = sb.events();
    assert!(events.contains("\treturn-requested\tbeam down in the agent terminal"));
    assert!(events.contains("\treturn-ready\t"), "{events}");
    assert!(!sb.tmux.alive(&format!("beam-{}", SandboxReturn::ID)));
    sb.bring_home();
}

#[test]
fn sandbox_beam_needs_a_transfer_id_when_several_transfers_exist() {
    let sb = SandboxReturn::new("");
    let other = sb.root.join("remote/88-2");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("return.sh"), "exit 0\n").unwrap();
    let out = sb.beam(&["down"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Give its ID"));
    assert!(!sb.stage.join("return-requested").exists());
    let missing = sb.beam(&["down", "99-9"]);
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no transfer 99-9"));
}
