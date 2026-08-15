//! Git hooks: gaff owns the dispatch.
//!
//! `gaff init --git` writes one script per declared hook into
//! `.git/hooks/`. Each script calls `gaff githook <name>`, which reads
//! the config and runs what that hook declares. The logic lives with
//! gaff, so one config covers the agent domain and the git domain.
//!
//! # Two contracts, not one
//!
//! The agent domain and the git domain block differently, and the
//! difference is deliberate.
//!
//! An agent hook must never block a session, so `gaff hook` exits 0 or
//! 1 and never 2. A git hook exists *to* block: a non-zero exit aborts
//! the commit or the push, and that is the point of a `pre-commit`
//! check. So `gaff githook` returns the failing command's status.
//!
//! # Why gaff writes `.git/hooks/` and not `core.hooksPath`
//!
//! `core.hooksPath` names one directory, so setting it silently
//! disables every hook another tool installed. gaff writes individual
//! files instead. When gaff finds a hook it did not write, it keeps
//! that file and calls it first, so an existing setup keeps working.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

/// The git hooks gaff knows how to install.
///
/// A hook that git calls with arguments gets them forwarded. `pre-push`
/// also receives its ref list on stdin, which gaff passes through.
pub const KNOWN_HOOKS: &[&str] = &[
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-push",
    "post-checkout",
    "post-merge",
    "pre-rebase",
];

/// One git-hook entry from the config.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHook {
    pub name: String,
    /// The git hooks this entry runs on, such as `pre-commit`. A name
    /// may carry the domain, as in `git:pre-commit`.
    pub on: Vec<String>,
    /// The argv to run. Unlike a handler, this may be a bare name,
    /// because the person who runs `gaff init --git` is asking for
    /// their own repo's tooling.
    pub command: Vec<String>,
    /// Skip the rest of the hook when this entry fails. The default is
    /// to stop, because a failing check should not be buried under the
    /// output of later checks.
    #[serde(default = "yes")]
    pub required: bool,
}

const fn yes() -> bool {
    true
}

impl GitHook {
    /// Whether this entry runs on `hook`.
    #[must_use]
    pub fn runs_on(&self, hook: &str) -> bool {
        self.on
            .iter()
            .any(|o| o.strip_prefix("git:").unwrap_or(o) == hook)
    }

    /// Problems that make the entry unusable.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.name.is_empty() {
            out.push("a git entry has no name".to_string());
        }
        if self.command.is_empty() {
            out.push(format!("git entry `{}`: the command is empty", self.name));
        }
        if self.on.is_empty() {
            out.push(format!("git entry `{}`: no hooks", self.name));
        }
        for hook in &self.on {
            let bare = hook.strip_prefix("git:").unwrap_or(hook);
            if !KNOWN_HOOKS.contains(&bare) {
                out.push(format!(
                    "git entry `{}`: gaff does not install `{bare}`. The known hooks are {}.",
                    self.name,
                    KNOWN_HOOKS.join(", ")
                ));
            }
        }
        out
    }
}

/// The hooks a config declares, in declaration order.
#[must_use]
pub fn hooks_declared(entries: &[GitHook]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in entries {
        for hook in &e.on {
            let bare = hook.strip_prefix("git:").unwrap_or(hook).to_string();
            if KNOWN_HOOKS.contains(&bare.as_str()) && !out.contains(&bare) {
                out.push(bare);
            }
        }
    }
    out
}

/// The script gaff writes for one hook.
fn script(hook: &str, command: &str) -> String {
    format!(
        "#!/bin/sh\n\
         {BANNER}\n\
         # gaff keeps a hook it did not write as {hook}.local and calls it first.\n\
         if [ -x \"$(dirname \"$0\")/{hook}.local\" ]; then\n\
         \t\"$(dirname \"$0\")/{hook}.local\" \"$@\" || exit $?\n\
         fi\n\
         exec {command} {hook} \"$@\"\n"
    )
}

/// The exact second line of a script gaff wrote. It is the ownership
/// marker, and it is compared whole.
const BANNER: &str = "# Installed by gaff. Edit .gaff/gaff.yml, then run `gaff init --git`.";

/// Whether gaff wrote this file.
///
/// The line must match exactly and sit where gaff puts it. A substring
/// test anywhere in the file would claim a foreign hook that merely
/// mentions gaff in a comment, and claiming it means overwriting it and
/// then deleting it on uninstall.
fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|s| s.lines().nth(1).is_some_and(|l| l == BANNER))
}

/// The directory git actually reads hooks from.
///
/// git resolves hooks against the *common* directory, not a worktree's
/// own git dir. A worktree's `.git` file points at
/// `<common>/worktrees/<name>`, and a hook written there is never run.
/// It reports as installed and silently does nothing, which is the
/// worst failure a blocking check can have.
#[must_use]
pub fn hooks_dir(cwd: &Path) -> Option<PathBuf> {
    // Ask git first. It knows about every layout, including ones this
    // function has not met.
    if let Ok(out) = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(cwd)
        .output()
        && out.status.success()
    {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("hooks"));
        }
    }

    let dot_git = cwd.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("hooks"));
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let rest = text.trim().strip_prefix("gitdir:")?.trim();
    let p = PathBuf::from(rest);
    let git_dir = if p.is_absolute() { p } else { cwd.join(p) };
    // Strip back to the common dir when this is a worktree's git dir.
    let common = git_dir
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "worktrees"))
        .and_then(Path::parent)
        .map_or_else(|| git_dir.clone(), Path::to_path_buf);
    Some(common.join("hooks"))
}

/// Install one script per declared hook.
///
/// A hook gaff did not write moves to `<hook>.local`, and the new
/// script calls it first. An install never discards another tool's
/// work.
pub fn install(cwd: &Path, entries: &[GitHook], command: &str) -> std::io::Result<Vec<String>> {
    // Resolve the command to this binary's absolute path. A bare name
    // would be resolved through PATH at commit time, and direnv, mise,
    // and node_modules/.bin all put a repo-controlled directory there.
    // A shadowed `gaff` would report success and run nothing.
    let command = &match (std::env::current_exe(), command.strip_prefix("gaff ")) {
        (Ok(exe), Some(rest)) => format!("{} {rest}", exe.display()),
        _ => command.to_string(),
    };
    let dir =
        hooks_dir(cwd).ok_or_else(|| std::io::Error::other("this is not a git repository"))?;
    std::fs::create_dir_all(&dir)?;
    let mut written = Vec::new();
    for hook in hooks_declared(entries) {
        let path = dir.join(&hook);
        if path.exists() && !is_ours(&path) {
            let kept = dir.join(format!("{hook}.local"));
            if kept.exists() {
                // Two foreign hooks and one slot to keep them in.
                // Overwriting loses the second one for good, so gaff
                // refuses and names both files.
                return Err(std::io::Error::other(format!(
                    "{hook} was written by another tool, and {hook}.local is already taken. \
                     gaff will not overwrite it. Move or merge {hook}.local, then run this again."
                )));
            }
            std::fs::rename(&path, &kept)?;
            eprintln!("gaff: kept the existing {hook} as {hook}.local, and calls it first.");
        }
        std::fs::write(&path, script(&hook, command))?;
        set_executable(&path)?;
        written.push(hook);
    }
    Ok(written)
}

/// Remove the scripts gaff wrote, and restore a kept hook.
pub fn uninstall(cwd: &Path) -> std::io::Result<Vec<String>> {
    let Some(dir) = hooks_dir(cwd) else {
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    for hook in KNOWN_HOOKS {
        let path = dir.join(hook);
        let kept = dir.join(format!("{hook}.local"));
        if path.exists() && is_ours(&path) {
            std::fs::remove_file(&path)?;
            if kept.exists() {
                std::fs::rename(&kept, &path)?;
            }
            removed.push((*hook).to_string());
        } else if kept.exists() && !path.exists() {
            // gaff's own hook is gone, and the file it displaced is
            // still parked. Put it back rather than leaving the user's
            // hook stranded forever.
            std::fs::rename(&kept, &path)?;
            removed.push(format!("{hook} (restored a kept hook)"));
        }
    }
    Ok(removed)
}

fn set_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Run the entries declared for one hook.
///
/// The exit code is the first failing command's code. A git hook exists
/// to block, so gaff reports the failure rather than swallowing it.
#[must_use]
pub fn run(cwd: &Path, entries: &[GitHook], hook: &str, args: &[String]) -> i32 {
    // A malformed entry must not vanish. Skipping it silently means a
    // check the operator declared stops running, and the commit lands
    // as though it passed.
    let mut broken = Vec::new();
    let due: Vec<&GitHook> = entries
        .iter()
        .filter(|e| e.runs_on(hook))
        .filter(|e| {
            let problems = e.problems();
            if problems.is_empty() {
                return true;
            }
            broken.extend(problems);
            false
        })
        .collect();
    if !broken.is_empty() {
        for p in &broken {
            eprintln!("gaff: {hook}: {p}");
        }
        eprintln!("gaff: {hook}: refusing, because a declared check could not run.");
        return 1;
    }
    if due.is_empty() {
        return 0;
    }
    let mut worst = 0;
    for entry in due {
        eprintln!("gaff: {hook}: {}", entry.name);
        let status = Command::new(&entry.command[0])
            .args(&entry.command[1..])
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::inherit())
            .status();
        let code = match status {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => {
                eprintln!("gaff: {hook}: `{}` did not start: {e}", entry.name);
                1
            }
        };
        if code != 0 {
            eprintln!("gaff: {hook}: `{}` failed with {code}", entry.name);
            if entry.required {
                return code;
            }
            worst = code;
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, on: &[&str], cmd: &[&str]) -> GitHook {
        GitHook {
            name: name.into(),
            on: on.iter().map(|s| (*s).to_string()).collect(),
            command: cmd.iter().map(|s| (*s).to_string()).collect(),
            required: true,
        }
    }

    fn repo(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gaff-git-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(d.join(".git/hooks")).unwrap();
        d
    }

    #[test]
    fn a_domain_prefix_is_accepted_on_a_hook_name() {
        let e = entry("fmt", &["git:pre-commit"], &["true"]);
        assert!(e.runs_on("pre-commit"));
        assert!(e.problems().is_empty(), "{:?}", e.problems());
    }

    #[test]
    fn an_unknown_hook_is_a_config_error() {
        let e = entry("x", &["pre-nothing"], &["true"]);
        assert!(
            e.problems().iter().any(|p| p.contains("does not install")),
            "{:?}",
            e.problems()
        );
    }

    #[test]
    fn install_writes_only_the_declared_hooks() {
        let d = repo("declared");
        let entries = [entry("fmt", &["pre-commit"], &["true"])];
        let written = install(&d, &entries, "gaff githook").unwrap();
        assert_eq!(written, vec!["pre-commit".to_string()]);
        assert!(d.join(".git/hooks/pre-commit").exists());
        assert!(
            !d.join(".git/hooks/pre-push").exists(),
            "nothing undeclared"
        );
    }

    #[test]
    fn install_keeps_a_hook_it_did_not_write_and_calls_it_first() {
        // Another tool's hook must survive. Overwriting it silently
        // would break that tool with no warning.
        let d = repo("keep");
        std::fs::write(
            d.join(".git/hooks/pre-commit"),
            "#!/bin/sh\necho OTHER TOOL\n",
        )
        .unwrap();
        let entries = [entry("fmt", &["pre-commit"], &["true"])];
        install(&d, &entries, "gaff githook").unwrap();
        let kept = std::fs::read_to_string(d.join(".git/hooks/pre-commit.local")).unwrap();
        assert!(kept.contains("OTHER TOOL"), "the original survives");
        let ours = std::fs::read_to_string(d.join(".git/hooks/pre-commit")).unwrap();
        assert!(ours.contains("pre-commit.local"), "and it is called first");
    }

    #[test]
    fn uninstall_restores_the_kept_hook() {
        let d = repo("restore");
        std::fs::write(
            d.join(".git/hooks/pre-commit"),
            "#!/bin/sh\necho OTHER TOOL\n",
        )
        .unwrap();
        let entries = [entry("fmt", &["pre-commit"], &["true"])];
        install(&d, &entries, "gaff githook").unwrap();
        let removed = uninstall(&d).unwrap();
        assert_eq!(removed, vec!["pre-commit".to_string()]);
        let back = std::fs::read_to_string(d.join(".git/hooks/pre-commit")).unwrap();
        assert!(back.contains("OTHER TOOL"), "the original is restored");
    }

    #[test]
    fn a_failing_required_entry_blocks_and_reports_its_code() {
        // A git hook exists to block. This is the opposite of the agent
        // path, which must never block a session.
        let d = repo("block");
        let entries = [entry("fail", &["pre-commit"], &["sh", "-c", "exit 3"])];
        assert_eq!(run(&d, &entries, "pre-commit", &[]), 3);
    }

    #[test]
    fn an_optional_entry_does_not_stop_the_later_ones() {
        let d = repo("optional");
        let mut soft = entry("soft", &["pre-commit"], &["sh", "-c", "exit 1"]);
        soft.required = false;
        let entries = [soft, entry("ok", &["pre-commit"], &["true"])];
        assert_eq!(
            run(&d, &entries, "pre-commit", &[]),
            1,
            "the failure is still reported"
        );
    }

    #[test]
    fn a_hook_with_no_entries_passes() {
        let d = repo("empty");
        assert_eq!(run(&d, &[], "pre-commit", &[]), 0);
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn repo(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gaff-own-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(d.join(".git/hooks")).unwrap();
        d
    }

    fn entry() -> GitHook {
        GitHook {
            name: "c".into(),
            on: vec!["pre-commit".into()],
            command: vec!["true".into()],
            required: true,
        }
    }

    #[test]
    fn a_second_foreign_hook_is_refused_rather_than_destroyed() {
        // gaff installs, another tool installs over it, gaff runs again.
        // The second tool's hook has nowhere to be kept, so gaff must
        // refuse instead of overwriting it.
        let d = repo("second");
        std::fs::write(d.join(".git/hooks/pre-commit"), "#!/bin/sh\necho ORIGINAL\n").unwrap();
        install(&d, &[entry()], "gaff githook").unwrap();
        std::fs::write(d.join(".git/hooks/pre-commit"), "#!/bin/sh\necho SECOND\n").unwrap();

        let err = install(&d, &[entry()], "gaff githook").unwrap_err();
        assert!(err.to_string().contains("already taken"), "{err}");
        let still = std::fs::read_to_string(d.join(".git/hooks/pre-commit")).unwrap();
        assert!(still.contains("SECOND"), "the second tool's hook survives");
        let kept = std::fs::read_to_string(d.join(".git/hooks/pre-commit.local")).unwrap();
        assert!(kept.contains("ORIGINAL"), "the first one is still kept");
    }

    #[test]
    fn a_foreign_hook_that_merely_mentions_gaff_is_not_claimed() {
        // A substring test would claim this file, overwrite it, and then
        // delete it on uninstall.
        let d = repo("mention");
        let foreign = "#!/bin/sh\n# replaces the one Installed by gaff, on purpose.\necho MINE\n";
        std::fs::write(d.join(".git/hooks/pre-commit"), foreign).unwrap();
        assert!(!is_ours(&d.join(".git/hooks/pre-commit")));

        install(&d, &[entry()], "gaff githook").unwrap();
        let kept = std::fs::read_to_string(d.join(".git/hooks/pre-commit.local")).unwrap();
        assert!(kept.contains("MINE"), "it was kept, not claimed");

        uninstall(&d).unwrap();
        let back = std::fs::read_to_string(d.join(".git/hooks/pre-commit")).unwrap();
        assert!(back.contains("MINE"), "and restored");
    }

    #[test]
    fn gaff_recognizes_its_own_script() {
        let d = repo("own");
        install(&d, &[entry()], "gaff githook").unwrap();
        assert!(is_ours(&d.join(".git/hooks/pre-commit")));
    }
}
