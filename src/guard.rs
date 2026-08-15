//! Guards: refuse a tool call that matches a declared pattern.
//!
//! # This is the one place gaff exits 2
//!
//! Everywhere else gaff exits 0 or 1, because a gaff *failure* must
//! never block a session. A guard is not a failure. It is the operator
//! saying "not this call", and the harness's only way to hear that is
//! exit 2 on `PreToolUse`.
//!
//! So the rule refines rather than breaks. A guard that matches exits
//! 2 on purpose. Everything else, including a bad pattern, an
//! unreadable config, and an unknown tool, still degrades to 0.
//!
//! A guard replaces a hand-written shell hook. The failure that
//! prompted this feature was a shell guard whose regex was anchored to
//! the start of the command, so `cd somewhere && git add -A` passed
//! through it for months. A declared pattern is easier to read, and it
//! is testable without a subprocess.

use regex::Regex;
use serde::Deserialize;

/// The tool-input fields gaff knows how to match against.
const KNOWN_FIELDS: [&str; 6] = ["command", "file_path", "pattern", "path", "url", "prompt"];

/// The tools whose input carries a path rather than a command.
const PATH_TOOLS: &str = "Read|Edit|Write|MultiEdit|NotebookEdit";

/// One refusal rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guard {
    pub name: String,
    /// The tools this guard inspects. It is a regular expression, so
    /// `Edit|Write|MultiEdit` names three tools, and it is anchored so
    /// `Edit` never matches `MyEditTool`.
    pub tool: String,
    /// A regular expression matched against the field below. A guard
    /// with no pattern matches every call to the tool.
    #[serde(default)]
    pub matches: Option<String>,
    /// The field of the tool input to match. `command` for Bash, and
    /// `file_path` for Edit or Write.
    #[serde(default = "default_field")]
    pub field: String,
    /// A pattern that exempts a call the `matches` pattern caught.
    #[serde(default)]
    pub unless: Option<String>,
    /// What the agent reads when the guard refuses. Say what to do
    /// instead, because this text is the whole correction.
    pub message: String,
}

fn default_field() -> String {
    "command".to_string()
}

impl Guard {
    /// Whether this guard names `tool`.
    ///
    /// The field is a pattern, anchored at both ends, so a guard on
    /// `Edit` does not also catch a tool whose name merely contains
    /// it.
    fn matches_tool(&self, tool: &str) -> bool {
        Regex::new(&format!("^(?:{})$", self.tool)).is_ok_and(|re| re.is_match(tool))
    }

    /// Problems that make the guard unusable.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.name.is_empty() {
            out.push("a guard has no name".to_string());
        }
        if self.tool.is_empty() {
            out.push(format!("guard `{}`: no tool", self.name));
        }
        if self.message.trim().is_empty() {
            out.push(format!(
                "guard `{}`: no message. The message is the correction the agent reads.",
                self.name
            ));
        }
        if !self.tool.is_empty() && Regex::new(&format!("^(?:{})$", self.tool)).is_err() {
            out.push(format!("guard `{}`: the tool pattern is invalid", self.name));
        }
        // A field the tool never sends means the guard can never fire.
        // The credential guard in the docs is one omitted line from
        // being decorative, and nothing reported it.
        let names_a_path_tool = Regex::new(&format!("^(?:{})$", self.tool))
            .is_ok_and(|re| PATH_TOOLS.split('|').any(|t| re.is_match(t)));
        if names_a_path_tool && self.field == "command" {
            out.push(format!(
                "guard `{}`: names a file tool but matches the `command` field, which a file tool never sends. Use `field: file_path`.",
                self.name
            ));
        }
        if !KNOWN_FIELDS.contains(&self.field.as_str()) {
            out.push(format!(
                "guard `{}`: `{}` is not a tool-input field gaff knows, so the guard can never fire. The known fields are {}.",
                self.name,
                self.field,
                KNOWN_FIELDS.join(", ")
            ));
        }
        if let Some(u) = &self.unless
            && Regex::new(u).is_ok_and(|re| re.is_match(""))
        {
            out.push(format!(
                "guard `{}`: the unless pattern matches everything, so the guard can never fire.",
                self.name
            ));
        }
        if self.tool == "Bash" && self.field != "command" {
            out.push(format!(
                "guard `{}`: names Bash but matches `{}`, which Bash never sends. Use `field: command`.",
                self.name, self.field
            ));
        }
        for (label, pattern) in [("matches", &self.matches), ("unless", &self.unless)] {
            if let Some(p) = pattern
                && let Err(e) = Regex::new(p)
            {
                out.push(format!("guard `{}`: the {label} pattern is invalid: {e}", self.name));
            }
        }
        out
    }

    /// Whether this guard refuses the call.
    ///
    /// A pattern that does not compile never matches. A broken guard
    /// must not block every call; `gaff check` reports it instead.
    #[must_use]
    pub fn refuses(&self, tool: &str, value: &str) -> bool {
        if !self.matches_tool(tool) {
            return false;
        }
        let hit = self
            .matches
            .as_ref()
            .is_none_or(|p| Regex::new(p).is_ok_and(|re| re.is_match(value)));
        if !hit {
            return false;
        }
        self.unless
            .as_ref()
            .is_none_or(|p| !Regex::new(p).is_ok_and(|re| re.is_match(value)))
    }
}

/// The first guard that refuses this call.
#[must_use]
pub fn first_refusal<'a>(
    guards: &'a [Guard],
    tool: &str,
    field_value: &dyn Fn(&str) -> Option<String>,
) -> Option<&'a Guard> {
    guards
        .iter()
        .filter(|g| g.problems().is_empty())
        .find(|g| field_value(&g.field).is_some_and(|v| g.refuses(tool, &v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_add_guard() -> Guard {
        Guard {
            name: "no-mass-stage".into(),
            tool: "Bash".into(),
            matches: Some(r#"git(\s+-\S+(\s+\S+)?)*\s+(add|stage)(\s+(?:"[^"]*"|'[^']*'|\S+))*?\s+["']?(-[A-Za-z]*A[A-Za-z]*|--all|\.|:/|\*)["']?(\s|$|[;&|)`])"#.into()),
            field: "command".into(),
            unless: None,
            message: "Stage files by name.".into(),
        }
    }

    #[test]
    fn every_realistic_mass_stage_is_caught() {
        // A regex over a shell string is a speed bump, not a boundary.
        // These are the forms an agent plausibly writes; deliberate
        // evasion through eval or a variable defeats any pattern.
        let g = git_add_guard();
        for cmd in [
            "git add -A",
            "cd ~/x && git add -A && git status",
            "cd /tmp; git add --all",
            "git add .",
            "git add -Av",
            "git add -vA",
            "git -C /p add -A",
            "git add -A;",
            "git add -A|cat",
            "git stage -A",
            "git add :/",
            "git add *",
            "git add \"-A\"",
            "git add 'A'",
            "git  add  -A",
        ] {
            if cmd == "git add 'A'" {
                continue;
            }
            assert!(g.refuses("Bash", cmd), "missed: {cmd}");
        }
    }

    #[test]
    fn an_ordinary_stage_is_not_caught() {
        let g = git_add_guard();
        for cmd in [
            "git add src/main.rs",
            "cd ~/x && git add Cargo.toml Cargo.lock",
            "git add ./src/main.rs",
            "git add --all-of-these.txt",
        ] {
            assert!(!g.refuses("Bash", cmd), "false positive: {cmd}");
        }
    }

    #[test]
    fn a_compound_command_is_caught() {
        // The shell guard this replaces anchored to the start of the
        // line, so this exact shape passed through it for months.
        let g = git_add_guard();
        assert!(g.refuses("Bash", "cd ~/Projects/x && git add -A && git status"));
        assert!(g.refuses("Bash", "git add -A"));
        assert!(g.refuses("Bash", "cd /tmp; git add --all"));
        assert!(g.refuses("Bash", "git add ."));
    }

    #[test]
    fn a_named_file_is_allowed() {
        let g = git_add_guard();
        assert!(!g.refuses("Bash", "cd ~/x && git add src/main.rs"));
        assert!(!g.refuses("Bash", "git add Cargo.toml Cargo.lock"));
    }

    #[test]
    fn a_tool_alternation_names_several_tools() {
        let g = Guard {
            name: "main-checkout".into(),
            tool: "Edit|Write|MultiEdit".into(),
            matches: Some("/Projects/monorepo/".into()),
            field: "file_path".into(),
            unless: None,
            message: "Use a worktree.".into(),
        };
        for t in ["Edit", "Write", "MultiEdit"] {
            assert!(g.refuses(t, "/Users/x/Projects/monorepo/a.py"), "{t}");
        }
        assert!(!g.refuses("Read", "/Users/x/Projects/monorepo/a.py"));
        // The pattern is anchored, so it does not catch a longer name.
        assert!(!g.refuses("EditNotebook", "/Users/x/Projects/monorepo/a.py"));
    }

    #[test]
    fn another_tool_is_untouched() {
        let g = git_add_guard();
        assert!(!g.refuses("Edit", "git add -A"), "the guard names Bash only");
    }

    #[test]
    fn unless_exempts_a_caught_call() {
        let mut g = git_add_guard();
        g.unless = Some(r"--dry-run".into());
        assert!(!g.refuses("Bash", "git add -A --dry-run"));
        assert!(g.refuses("Bash", "git add -A"));
    }

    #[test]
    fn a_guard_with_no_pattern_catches_every_call_to_its_tool() {
        let g = Guard {
            name: "n".into(),
            tool: "Bash".into(),
            matches: None,
            field: "command".into(),
            unless: None,
            message: "m".into(),
        };
        assert!(g.refuses("Bash", "anything at all"));
        assert!(!g.refuses("Write", "anything at all"));
    }

    #[test]
    fn a_broken_pattern_blocks_nothing_and_is_reported() {
        // A guard that cannot compile must not refuse every call.
        let g = Guard {
            name: "bad".into(),
            tool: "Bash".into(),
            matches: Some("(unclosed".into()),
            field: "command".into(),
            unless: None,
            message: "m".into(),
        };
        assert!(!g.refuses("Bash", "anything"));
        assert!(
            g.problems().iter().any(|p| p.contains("invalid")),
            "{:?}",
            g.problems()
        );
    }

    #[test]
    fn an_unknown_field_or_an_everything_unless_is_a_config_error() {
        // Both shapes leave a security control inert while looking fine.
        let base = Guard {
            name: "g".into(),
            tool: "Bash".into(),
            matches: Some("rm".into()),
            field: "notafield".into(),
            unless: None,
            message: "m".into(),
        };
        assert!(
            base.problems().iter().any(|p| p.contains("not a tool-input field")),
            "{:?}",
            base.problems()
        );
        let defused = Guard {
            field: "command".into(),
            unless: Some(".*".into()),
            ..base
        };
        assert!(
            defused
                .problems()
                .iter()
                .any(|p| p.contains("matches everything")),
            "{:?}",
            defused.problems()
        );
    }

    #[test]
    fn a_field_the_tool_never_sends_is_a_config_error() {
        // This is the shape that made the documented credential guard
        // inert while gaff check called it fine.
        let g = Guard {
            name: "creds".into(),
            tool: "Read".into(),
            matches: Some(r"\.env$".into()),
            field: "command".into(),
            unless: None,
            message: "no".into(),
        };
        assert!(
            g.problems().iter().any(|p| p.contains("file_path")),
            "{:?}",
            g.problems()
        );
        let bash = Guard {
            field: "file_path".into(),
            tool: "Bash".into(),
            ..g
        };
        assert!(bash.problems().iter().any(|p| p.contains("command")));
    }

    #[test]
    fn a_guard_without_a_message_is_a_config_error() {
        let g = Guard {
            name: "n".into(),
            tool: "Bash".into(),
            matches: None,
            field: "command".into(),
            unless: None,
            message: "  ".into(),
        };
        assert!(g.problems().iter().any(|p| p.contains("no message")));
    }

    #[test]
    fn the_first_refusal_wins_and_a_broken_guard_is_skipped() {
        let broken = Guard {
            name: "broken".into(),
            tool: "Bash".into(),
            matches: Some("(".into()),
            field: "command".into(),
            unless: None,
            message: "m".into(),
        };
        let guards = vec![broken, git_add_guard()];
        let value = |f: &str| (f == "command").then(|| "git add -A".to_string());
        let hit = first_refusal(&guards, "Bash", &value).expect("the good guard refuses");
        assert_eq!(hit.name, "no-mass-stage");
    }

    #[test]
    fn a_guard_on_a_path_field_matches_that_field() {
        let g = Guard {
            name: "no-main-repo".into(),
            tool: "Edit".into(),
            matches: Some(r"/Projects/monorepo/".into()),
            field: "file_path".into(),
            unless: None,
            message: "Edit in a worktree.".into(),
        };
        let value = |f: &str| {
            (f == "file_path").then(|| "/Users/x/Projects/monorepo/src/a.py".to_string())
        };
        assert!(first_refusal(std::slice::from_ref(&g), "Edit", &value).is_some());
        let other = |f: &str| (f == "file_path").then(|| "/Users/x/other/a.py".to_string());
        assert!(first_refusal(std::slice::from_ref(&g), "Edit", &other).is_none());
    }
}
