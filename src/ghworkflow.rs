//! The github domain: generate workflows, and check them for drift.
//!
//! gaff cannot run a GitHub event. It is not present when one fires.
//! So this domain is generated, not executed: gaff renders a workflow
//! from the config, and `gaff check --github` compares the render
//! against the file that is committed.
//!
//! The point is one declaration. A check you run in `pre-commit` and a
//! check you run in CI should be the same command, written once. A
//! step may name a git entry with `use`, and gaff renders that entry's
//! command into the workflow.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One workflow to generate.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    /// The workflow name. It also names the file:
    /// `.github/workflows/<name>.yml`.
    pub name: String,
    /// The GitHub events that trigger it, such as `push` or
    /// `pull_request`. A name may carry the domain, as in
    /// `github:push`.
    pub on: Vec<String>,
    /// Restrict the trigger to these branches.
    #[serde(default)]
    pub branches: Vec<String>,
    #[serde(default = "default_runner")]
    pub runs_on: String,
    pub steps: Vec<Step>,
}

fn default_runner() -> String {
    "ubuntu-latest".to_string()
}

/// One step. It carries a command, or it names a git entry to reuse.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub name: Option<String>,
    /// The argv to run.
    #[serde(default)]
    pub command: Vec<String>,
    /// The name of a `git:` entry whose command to reuse. This is how
    /// one check runs in a git hook and in CI without being written
    /// twice.
    #[serde(default)]
    pub use_git: Option<String>,
}

/// The events gaff knows how to render a trigger for.
pub const KNOWN_EVENTS: &[&str] = &[
    "push",
    "pull_request",
    "merge_group",
    "workflow_dispatch",
    "schedule",
    "release",
];

impl Workflow {
    /// Problems that make the workflow unusable.
    #[must_use]
    pub fn problems(&self, git: &[crate::githook::GitHook]) -> Vec<String> {
        let mut out = Vec::new();
        // The name becomes both a filename and a GitHub job id. A job
        // id must start with a letter or an underscore and hold only
        // alphanumerics, a dash, or an underscore.
        let valid_id = !self.name.is_empty()
            && self
                .name
                .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid_id {
            out.push(format!(
                "workflow `{}`: the name becomes a filename and a job id, so it must start with a letter or an underscore and hold only letters, digits, a dash, or an underscore",
                self.name
            ));
        }
        if self.on.is_empty() {
            out.push(format!("workflow `{}`: no events", self.name));
        }
        for event in &self.on {
            let bare = event.strip_prefix("github:").unwrap_or(event);
            if !KNOWN_EVENTS.contains(&bare) {
                out.push(format!(
                    "workflow `{}`: gaff does not render `{bare}`. The known events are {}.",
                    self.name,
                    KNOWN_EVENTS.join(", ")
                ));
            }
        }
        if self.steps.is_empty() {
            out.push(format!("workflow `{}`: no steps", self.name));
        }
        for step in &self.steps {
            match (&step.use_git, step.command.is_empty()) {
                (Some(name), true) => {
                    if !git.iter().any(|g| &g.name == name) {
                        out.push(format!(
                            "workflow `{}`: no git entry named `{name}` to reuse",
                            self.name
                        ));
                    }
                }
                (None, false) => {}
                (Some(_), false) => out.push(format!(
                    "workflow `{}`: a step sets both `command` and `use_git`",
                    self.name
                )),
                (None, true) => out.push(format!(
                    "workflow `{}`: a step has neither `command` nor `use_git`",
                    self.name
                )),
            }
        }
        out
    }

    /// The path this workflow renders to.
    #[must_use]
    pub fn path(&self, cwd: &Path) -> PathBuf {
        cwd.join(".github/workflows")
            .join(format!("{}.yml", self.name))
    }
}

/// Quote a value so it is always a valid YAML scalar.
///
/// A plain scalar ends at a `#`, and a leading `-` or an embedded `:`
/// changes the node type. Single quotes make the value literal, and a
/// contained quote doubles.
fn yaml_scalar(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

/// Quote one argv element for a shell `run:` line.
fn shell_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+".contains(c));
    if safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Render a workflow to YAML.
///
/// The output is deterministic, because it is committed and read in a
/// diff. The same config always renders the same bytes.
#[must_use]
pub fn render(wf: &Workflow, git: &[crate::githook::GitHook]) -> String {
    let mut out = String::new();
    out.push_str("# Generated by gaff from the gaff config. Do not edit.\n");
    out.push_str("# Change the config, then run `gaff init --github`.\n");
    out.push_str("# `gaff check --github` reports a file that drifted.\n");
    let _ = write!(out, "name: {}\n\non:\n", yaml_scalar(&wf.name));

    for event in &wf.on {
        let bare = event.strip_prefix("github:").unwrap_or(event);
        // Only the branch-filtered events take a branches key.
        if wf.branches.is_empty() || !matches!(bare, "push" | "pull_request") {
            let _ = writeln!(out, "  {bare}:");
        } else {
            let _ = write!(out, "  {bare}:\n    branches:\n");
            for b in &wf.branches {
                let _ = writeln!(out, "      - {}", yaml_scalar(b));
            }
        }
    }

    let _ = write!(
        out,
        "\njobs:\n  {}:\n    runs-on: {}\n    steps:\n      - uses: actions/checkout@v4\n",
        wf.name,
        yaml_scalar(&wf.runs_on)
    );

    for step in &wf.steps {
        let (label, argv) = step.use_git.as_ref().map_or_else(
            || {
                (
                    step.name.clone().unwrap_or_else(|| "run".to_string()),
                    step.command.clone(),
                )
            },
            |name| {
                let argv = git
                    .iter()
                    .find(|g| &g.name == name)
                    .map(|g| g.command.clone())
                    .unwrap_or_default();
                (step.name.clone().unwrap_or_else(|| name.clone()), argv)
            },
        );
        let line = argv
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        // A block scalar cannot be broken by a `#`, a quote, or a
        // newline in the command. A plain scalar ends at " #", which
        // silently truncated the step.
        let _ = write!(
            out,
            "      - name: {}\n        run: |\n",
            yaml_scalar(&label)
        );
        for l in line.split('\n') {
            let _ = writeln!(out, "          {l}");
        }
    }
    out
}

/// What a check found for one workflow.
#[derive(Debug, PartialEq, Eq)]
pub enum Drift {
    /// The file matches the render.
    Match,
    /// The file is absent.
    Missing,
    /// The file differs from the render.
    Differs,
}

/// Compare the render against the committed file.
#[must_use]
pub fn drift(wf: &Workflow, git: &[crate::githook::GitHook], cwd: &Path) -> Drift {
    let path = wf.path(cwd);
    match std::fs::read_to_string(&path) {
        Err(_) => Drift::Missing,
        Ok(on_disk) if on_disk == render(wf, git) => Drift::Match,
        Ok(_) => Drift::Differs,
    }
}

/// Write every workflow. Returns the paths written.
pub fn write_all(
    cwd: &Path,
    workflows: &[Workflow],
    git: &[crate::githook::GitHook],
) -> std::io::Result<Vec<String>> {
    let dir = cwd.join(".github/workflows");
    std::fs::create_dir_all(&dir)?;
    let mut written = Vec::new();
    for wf in workflows {
        let path = wf.path(cwd);
        std::fs::write(&path, render(wf, git))?;
        written.push(format!(".github/workflows/{}.yml", wf.name));
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::githook::GitHook;

    fn git_entry(name: &str, cmd: &[&str]) -> GitHook {
        GitHook {
            name: name.into(),
            on: vec!["pre-commit".into()],
            command: cmd.iter().map(|s| (*s).to_string()).collect(),
            required: true,
        }
    }

    fn wf() -> Workflow {
        Workflow {
            name: "ci".into(),
            on: vec!["push".into(), "github:pull_request".into()],
            branches: vec!["main".into()],
            runs_on: "ubuntu-latest".into(),
            steps: vec![
                Step {
                    name: None,
                    command: vec![],
                    use_git: Some("fmt".into()),
                },
                Step {
                    name: Some("test".into()),
                    command: vec!["cargo".into(), "test".into()],
                    use_git: None,
                },
            ],
        }
    }

    #[test]
    fn a_step_reuses_the_git_entry_command() {
        // One check, written once, running in both places.
        let git = [git_entry("fmt", &["cargo", "fmt", "--check"])];
        let out = render(&wf(), &git);
        assert!(
            out.contains("- name: 'fmt'\n        run: |\n          cargo fmt --check"),
            "{out}"
        );
        assert!(
            out.contains("- name: 'test'\n        run: |\n          cargo test"),
            "{out}"
        );
    }

    #[test]
    fn the_domain_prefix_is_accepted_on_an_event() {
        let out = render(&wf(), &[]);
        assert!(out.contains("  push:\n"), "{out}");
        assert!(out.contains("  pull_request:\n"), "{out}");
        assert!(
            !out.contains("github:"),
            "the prefix never reaches the file"
        );
    }

    #[test]
    fn branches_filter_only_the_events_that_take_them() {
        let mut w = wf();
        w.on = vec!["push".into(), "workflow_dispatch".into()];
        let out = render(&w, &[]);
        assert!(
            out.contains("  push:\n    branches:\n      - 'main'"),
            "{out}"
        );
        assert!(
            out.contains("  workflow_dispatch:\n"),
            "a dispatch takes no branch filter: {out}"
        );
    }

    #[test]
    fn the_render_is_deterministic() {
        let git = [git_entry("fmt", &["cargo", "fmt"])];
        assert_eq!(render(&wf(), &git), render(&wf(), &git));
    }

    #[test]
    fn an_argument_that_needs_quoting_gets_it() {
        let mut w = wf();
        w.steps = vec![Step {
            name: Some("s".into()),
            command: vec!["sh".into(), "-c".into(), "echo hi && exit 0".into()],
            use_git: None,
        }];
        let out = render(&w, &[]);
        assert!(
            out.contains("run: |\n          sh -c 'echo hi && exit 0'"),
            "{out}"
        );
    }

    #[test]
    fn a_hash_or_a_newline_in_a_command_stays_valid_yaml() {
        // A plain scalar ends at " #", which silently truncated the
        // step and committed a broken command.
        let mut w = wf();
        w.steps = vec![Step {
            name: Some("fix".into()),
            command: vec!["echo".into(), "fix #123 now".into()],
            use_git: None,
        }];
        let out = render(&w, &[]);
        assert!(
            out.contains("run: |\n          echo 'fix #123 now'"),
            "{out}"
        );

        w.steps = vec![Step {
            name: Some("multi".into()),
            command: vec!["sh".into(), "-c".into(), "echo a\necho b".into()],
            use_git: None,
        }];
        let out = render(&w, &[]);
        for line in out
            .lines()
            .skip_while(|l| !l.contains("run: |"))
            .skip(1)
            .take(2)
        {
            assert!(
                line.starts_with("          "),
                "a continuation stays indented: {line:?}"
            );
        }
    }

    #[test]
    fn a_colon_or_a_dash_in_a_name_is_quoted() {
        let mut w = wf();
        w.steps = vec![Step {
            name: Some("lint: rust".into()),
            command: vec!["true".into()],
            use_git: None,
        }];
        assert!(render(&w, &[]).contains("- name: 'lint: rust'"));
        w.steps[0].name = Some("- rm -rf /".into());
        assert!(render(&w, &[]).contains("- name: '- rm -rf /'"));
    }

    #[test]
    fn a_job_id_must_be_a_valid_github_identifier() {
        for bad in ["my ci", "8ball", "$(whoami)", "has.dot", "a/b", ""] {
            let mut w = wf();
            w.name = bad.into();
            assert!(
                w.problems(&[]).iter().any(|p| p.contains("job id")),
                "{bad:?} must be rejected"
            );
        }
        let mut ok = wf();
        ok.name = "ci-2_x".into();
        assert!(
            !ok.problems(&[]).iter().any(|p| p.contains("job id")),
            "an ordinary name is fine"
        );
    }

    #[test]
    fn drift_reports_missing_then_match_then_differs() {
        let dir = std::env::temp_dir().join(format!("gaff-gh-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let git = [git_entry("fmt", &["cargo", "fmt"])];
        let w = wf();

        assert_eq!(drift(&w, &git, &dir), Drift::Missing);
        write_all(&dir, std::slice::from_ref(&w), &git).unwrap();
        assert_eq!(drift(&w, &git, &dir), Drift::Match);
        std::fs::write(w.path(&dir), "name: edited by hand\n").unwrap();
        assert_eq!(drift(&w, &git, &dir), Drift::Differs);
    }

    #[test]
    fn a_step_naming_an_absent_git_entry_is_an_error() {
        let problems = wf().problems(&[]);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("no git entry named `fmt`")),
            "{problems:?}"
        );
    }

    #[test]
    fn an_unknown_event_and_a_bad_name_are_errors() {
        let mut w = wf();
        w.name = "has.dot".into();
        w.on = vec!["pre_merge".into()];
        let problems = w.problems(&[]);
        assert!(
            problems.iter().any(|p| p.contains("job id")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("does not render")),
            "{problems:?}"
        );
    }
}
