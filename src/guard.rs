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

/// The fields each known tool actually sends.
///
/// A guard matched against a field its tool never sends can never fire,
/// and it reads exactly like a working rule. The check used to cover
/// Bash and the file tools only, so every other pairing was blessed:
/// `Grep` with `command`, `Glob` with `file_path`, `WebFetch` with
/// `command`. A table covers them all and, unlike the old two-case
/// test, names the right half of an alternation as the dead one.
///
/// A tool with no matchable field is listed with an empty slice. A
/// guard on it can still match every call, which is a legitimate way to
/// refuse a tool outright.
const TOOL_FIELDS: [(&str, &[&str]); 14] = [
    ("Bash", &["command"]),
    ("BashOutput", &[]),
    ("Read", &["file_path"]),
    ("Edit", &["file_path"]),
    ("Write", &["file_path"]),
    ("MultiEdit", &["file_path"]),
    ("NotebookEdit", &["file_path"]),
    ("Glob", &["pattern", "path"]),
    ("Grep", &["pattern", "path"]),
    ("WebFetch", &["url", "prompt"]),
    ("WebSearch", &["query"]),
    ("Task", &["prompt"]),
    ("TodoWrite", &[]),
    ("KillShell", &[]),
];

/// The tool-input fields gaff knows how to match against.
const KNOWN_FIELDS: [&str; 7] = [
    "command",
    "file_path",
    "pattern",
    "path",
    "url",
    "prompt",
    "query",
];

/// Commands a self-defusing `unless` is tested against.
///
/// An `unless` that exempts everything makes the guard decorative. The
/// probes are realistic values rather than the empty string: a
/// zero-width assertion such as `\b` finds no boundary in an empty
/// string, so an empty-string test passed it while it exempted every
/// real command.
const UNLESS_PROBES: [&str; 4] = [
    "git add -A",
    "rm -rf /",
    "/etc/passwd",
    "echo hello world",
];

/// The distinct values of `items`, in first-seen order.
fn dedup(items: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        if !out.iter().any(|o| o == i) {
            out.push((*i).to_string());
        }
    }
    out
}

/// The literal text a pattern must match, when it is plain enough to
/// say.
///
/// Returns `None` for anything with real regex structure. A group or an
/// alternation means part of the pattern is optional, and then a
/// substring of the pattern text is not necessarily present in a string
/// the pattern matches: `a(b|c)d` matches `acd`, which holds no `b`.
/// Only patterns with no such structure yield a core.
fn literal_core(pattern: &str) -> Option<String> {
    let body = pattern
        .strip_prefix('^')
        .unwrap_or(pattern)
        .strip_suffix('$')
        .unwrap_or_else(|| pattern.strip_prefix('^').unwrap_or(pattern));
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                // An escaped metacharacter stands for itself.
                Some(n) if !n.is_alphanumeric() => out.push(n),
                // `\d`, `\b`, `\s` and friends are structure.
                _ => return None,
            }
            continue;
        }
        if ".[]|()*+?{}^$".contains(c) {
            return None;
        }
        out.push(c);
    }
    (!out.is_empty()).then_some(out)
}

/// Why a pattern can never match, when that is decidable cheaply.
///
/// The regex crate exposes no emptiness oracle, so this covers the two
/// shapes an operator actually writes: an end anchor in the middle of a
/// pattern, and a word boundary between two word characters. Both read
/// as working rules and match nothing.
fn unsatisfiable(pattern: &str) -> Option<String> {
    if pattern.contains("(?m") {
        return None;
    }
    let bytes: Vec<char> = pattern.chars().collect();
    for (i, c) in bytes.iter().enumerate() {
        let escaped = i > 0 && bytes[i - 1] == '\\';
        // `x$y`: nothing follows the end of the text.
        //
        // An end anchor closing an alternation branch is ordinary and
        // common — `($|[^a-z])` is exactly how a terminator is
        // written — so only a literal following the anchor counts.
        if *c == '$'
            && !escaped
            && let Some(next) = bytes.get(i + 1)
            && !")|*?+{".contains(*next)
        {
            return Some("`$` is the end of the text and something follows it".to_string());
        }
        // `a\bb`: there is no word boundary between two word characters.
        if *c == 'b'
            && escaped
            && i >= 2
            && i + 1 < bytes.len()
            && bytes[i - 2].is_alphanumeric()
            && bytes[i + 1].is_alphanumeric()
        {
            return Some(
                "`\\b` needs a word boundary and it sits between two word characters".to_string(),
            );
        }
    }
    if let Some(at) = pattern.find("\\z")
        && !pattern[at + 2..].is_empty()
        && !pattern[at + 2..].starts_with([')', '|'])
    {
        return Some("`\\z` is the end of the text and something follows it".to_string());
    }
    None
}

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
        //
        // This is a hard failure only when EVERY tool the pattern names
        // is dead on the field. A pattern naming several tools where
        // one is live still works, and that case is a warning instead.
        if self.matches.is_some()
            && let Ok(re) = Regex::new(&format!("^(?:{})$", self.tool))
        {
            let named: Vec<&(&str, &[&str])> =
                TOOL_FIELDS.iter().filter(|(t, _)| re.is_match(t)).collect();
            let live = |fields: &[&str]| fields.contains(&self.field.as_str());
            if !named.is_empty() && !named.iter().any(|(_, f)| live(f)) {
                let expected: Vec<&str> = named
                    .iter()
                    .flat_map(|(_, f)| f.iter().copied())
                    .collect();
                let hint = if expected.is_empty() {
                    "That tool sends no field gaff can match. Drop `matches` to refuse every call to it.".to_string()
                } else {
                    format!("Use one of {}.", dedup(&expected).join(", "))
                };
                out.push(format!(
                    "guard `{}`: matches the `{}` field, which {} never sends, so the guard can never fire. {hint}",
                    self.name,
                    self.field,
                    dedup(&named.iter().map(|(t, _)| *t).collect::<Vec<_>>()).join(" or ")
                ));
            }
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
            && Regex::new(u).is_ok_and(|re| UNLESS_PROBES.iter().all(|p| re.is_match(p)))
        {
            out.push(format!(
                "guard `{}`: the unless pattern matches everything, so the guard can never fire.",
                self.name
            ));
        }
        // An `unless` that covers everything `matches` catches makes
        // the guard decorative, and the probe corpus cannot see it:
        // `matches: 'git add'` with `unless: 'git'` exempts all of its
        // own hits, and no probe in the corpus has to contain `git`.
        // Compare the two patterns directly instead. This is a
        // heuristic over literal text, so it reports only the cases it
        // is certain of and stays quiet otherwise.
        if let (Some(m), Some(u)) = (&self.matches, &self.unless)
            && let Some(core) = literal_core(m)
            && u.split('|')
                .filter(|a| !a.is_empty())
                .any(|alt| literal_core(alt).is_some_and(|a| core.contains(&a)))
        {
            out.push(format!(
                "guard `{}`: the unless pattern `{u}` covers everything `{m}` catches, so the guard can never fire.",
                self.name
            ));
        }
        if let Some(m) = &self.matches
            && let Some(reason) = unsatisfiable(m)
        {
            out.push(format!(
                "guard `{}`: the matches pattern can never match, because {reason}.",
                self.name
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

    /// Things worth saying that do not make the guard invalid.
    ///
    /// A tool name gaff has never seen is almost always a typo, and a
    /// typo disarms the guard in silence. It is not an error, because a
    /// host gaff has not met yet may send a name this list lacks.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.tool.is_empty() {
            return out;
        }
        let Ok(re) = Regex::new(&format!("^(?:{})$", self.tool)) else {
            return out;
        };
        let named: Vec<&(&str, &[&str])> =
            TOOL_FIELDS.iter().filter(|(t, _)| re.is_match(t)).collect();
        if named.is_empty() {
            out.push(format!(
                "guard `{}`: the tool pattern `{}` names no tool gaff has seen, so the guard may never fire. Check the spelling.",
                self.name, self.tool
            ));
            return out;
        }
        // A pattern naming several tools can be live for some and dead
        // for the rest. Name the dead ones, and only when at least one
        // is live — otherwise `problems` already reported it as fatal.
        if self.matches.is_none() {
            return out;
        }
        let dead: Vec<&str> = named
            .iter()
            .filter(|(_, f)| !f.contains(&self.field.as_str()))
            .map(|(t, _)| *t)
            .collect();
        if !dead.is_empty() && dead.len() < named.len() {
            out.push(format!(
                "guard `{}`: matches the `{}` field, which {} never sends, so that part of the pattern can never fire.",
                self.name,
                self.field,
                dedup(&dead).join(" and ")
            ));
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
        // A guard with no pattern refuses every call to its tool, and a
        // call that omits the field is still a call. Requiring the
        // field here meant such a guard silently passed anything whose
        // payload did not carry it, and made a tool that sends no
        // matchable field impossible to refuse at all. The unit test
        // asserted on `refuses` directly, one layer below this, so it
        // passed while the shipped behaviour differed.
        .find(|g| {
            field_value(&g.field).map_or_else(
                || g.matches.is_none() && g.matches_tool(tool),
                |v| g.refuses(tool, &v),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_add_guard() -> Guard {
        Guard {
            name: "no-mass-stage".into(),
            tool: "Bash".into(),
            matches: Some(r#"git(\s+-[^\s"';&|()<>]+(\s+[^\s"';&|()<>]+)?)*\s+(add|stage)(\s+(?:"[^"]*"|'[^']*'|[^\s"';&|()<>]+))*?\s+["']?(-[A-Za-z]*A[A-Za-z]*|--all|\.\.?/*\*?|:/\.?|:\(top\)|\*)["']?($|[^A-Za-z0-9_/.-])"#.into()),
            field: "command".into(),
            unless: None,
            message: "Stage files by name.".into(),
        }
    }

    /// The example a reader copies must be the pattern the tests
    /// exercise.
    ///
    /// These drifted once. The tests were strengthened and the docs
    /// were not, so `gaff docs configuration` kept serving a pattern
    /// that missed 21 of 30 mass-stage forms while the suite passed.
    /// The docs ship inside the binary, so this compares against the
    /// bundled copy rather than the file on disk.
    #[test]
    fn the_documented_pattern_is_the_tested_pattern() {
        let doc = crate::docs::topic("configuration")
            .expect("the configuration page ships in the binary");
        let line = doc
            .lines()
            .find(|l| l.trim_start().starts_with("matches: 'git"))
            .expect("the docs show a mass-stage guard");
        // Unwrap the YAML single-quoted scalar: strip the delimiters,
        // then undouble the escaped quotes.
        let scalar = line.trim().trim_start_matches("matches: ").trim();
        let inner = scalar
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .expect("the example is a single-quoted YAML scalar");
        let documented = inner.replace("''", "'");
        assert_eq!(
            documented,
            git_add_guard().matches.unwrap(),
            "the documented mass-stage pattern drifted from the tested one"
        );
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
            "git add '-A'",
            "git  add  -A",
            // Redirection and punctuation terminators. The old
            // terminator class enumerated shell metacharacters and
            // missed the redirection family entirely.
            "git add -A>/dev/null",
            "git add -A>>log",
            "git add -A<x",
            "git add -A}",
            "git add -A]",
            "git add -A,",
            "git add -A:",
            "git add -A&",
            "git add -A`",
            // Pathspecs derived from `.`, which the previous
            // terminator class could not end because it excluded `/`
            // and `.` as path characters.
            "git add ./",
            "git add ./*",
            "git add .//",
            "git add ..",
            "git add ../",
            // git pathspec magic, which stages from the repo root.
            "git add :(top)",
            "git add :/.",
            // Separators must stop the scan. The argument list of one
            // command must not be read across `&&`, `;`, or `|` into
            // the next, and a quoted argument must not be split open.
            "cd /x && git add ./",
            "git add -v ./",
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

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn g(tool: &str, field: &str, unless: Option<&str>) -> Guard {
        Guard {
            name: "g".into(),
            tool: tool.into(),
            matches: Some("x".into()),
            field: field.into(),
            unless: unless.map(Into::into),
            message: "no".into(),
        }
    }

    #[test]
    fn a_misspelled_tool_name_is_reported() {
        // A typo here disarms the guard in silence, and it is the
        // likeliest operator error.
        assert!(!g("NoSuchTool", "command", None).warnings().is_empty());
        assert!(!g("bash", "command", None).warnings().is_empty());
        assert!(g("Bash", "command", None).warnings().is_empty());
    }

    #[test]
    fn a_tool_pattern_naming_a_host_gaff_has_not_met_is_a_warning_not_a_failure() {
        // A new host may send a name this build has never seen. The
        // guard must stay expressible, so this reports and does not
        // refuse.
        assert!(g("SomeFutureTool", "command", None).problems().is_empty());
    }

    #[test]
    fn a_dead_half_of_an_alternation_is_reported() {
        // `Bash|Read` on file_path guards Read and does nothing at all
        // for Bash.
        let w = g("Bash|Read", "file_path", None).warnings();
        assert!(w.iter().any(|m| m.contains("Bash")), "{w:?}");
    }

    #[test]
    fn a_zero_width_unless_that_exempts_everything_is_caught() {
        // `\b` does not match at position 0 of an empty string, so the
        // empty-string probe alone let it through while it exempted
        // every real command.
        for pattern in [r"\b", ".*", "", r"^", r"(?s).*"] {
            let problems = g("Bash", "command", Some(pattern)).problems();
            assert!(
                problems.iter().any(|p| p.contains("matches everything")),
                "pattern {pattern:?} should be caught, got {problems:?}"
            );
        }
    }

    #[test]
    fn a_real_unless_is_not_caught() {
        for pattern in ["--dry-run", "^git status", r"\.test\.py$"] {
            assert!(
                g("Bash", "command", Some(pattern)).problems().is_empty(),
                "pattern {pattern:?} is legitimate"
            );
        }
    }
}

#[cfg(test)]
mod inert_tests {
    use super::*;

    fn g(tool: &str, field: &str, matches: Option<&str>, unless: Option<&str>) -> Guard {
        Guard {
            name: "g".into(),
            tool: tool.into(),
            matches: matches.map(Into::into),
            field: field.into(),
            unless: unless.map(Into::into),
            message: "no".into(),
        }
    }

    #[test]
    fn a_guard_with_no_pattern_refuses_a_tool_that_sends_no_field() {
        // The docs promise this is how a tool is refused outright. The
        // field lookup sat above `refuses`, so the guard was skipped
        // whenever the payload carried no matching field, and a tool
        // with no matchable field could not be guarded at all.
        for tool in ["TodoWrite", "BashOutput", "KillShell"] {
            let guard = g(tool, "command", None, None);
            assert!(
                guard.problems().is_empty(),
                "{tool}: {:?}",
                guard.problems()
            );
            let none = |_: &str| None;
            assert!(
                first_refusal(std::slice::from_ref(&guard), tool, &none).is_some(),
                "{tool} must be refusable"
            );
        }
    }

    #[test]
    fn a_pattern_less_guard_refuses_a_call_that_omits_the_field() {
        let guard = g("Bash", "command", None, None);
        let absent = |_: &str| None;
        assert!(first_refusal(std::slice::from_ref(&guard), "Bash", &absent).is_some());
        // And it still only names its own tool.
        assert!(first_refusal(std::slice::from_ref(&guard), "Read", &absent).is_none());
    }

    #[test]
    fn an_unless_that_covers_its_own_matches_is_caught() {
        for (m, u) in [
            ("git add", "git"),
            ("rm -rf", "rm|ls|cd"),
            ("rm -rf", "rm -rf"),
            (r"\.pem$", "pem"),
        ] {
            let problems = g("Bash", "command", Some(m), Some(u)).problems();
            assert!(
                problems.iter().any(|p| p.contains("covers everything")),
                "matches {m:?} unless {u:?} should be caught, got {problems:?}"
            );
        }
    }

    #[test]
    fn a_real_unless_is_left_alone() {
        for (m, u) in [
            ("git add", "--dry-run"),
            (r"\.pem$", "example"),
            ("rm -rf", "--help"),
        ] {
            let problems = g("Bash", "command", Some(m), Some(u)).problems();
            assert!(
                problems.is_empty(),
                "matches {m:?} unless {u:?} is legitimate, got {problems:?}"
            );
        }
    }

    #[test]
    fn a_pattern_that_can_never_match_is_caught() {
        for m in ["x$y", r"a\bb", r"\Ax\z\Ay"] {
            let problems = g("Bash", "command", Some(m), None).problems();
            assert!(
                problems.iter().any(|p| p.contains("can never match")),
                "{m:?} should be caught, got {problems:?}"
            );
        }
    }

    #[test]
    fn an_end_anchor_closing_an_alternation_is_not_flagged() {
        // `($|[^a-z])` is how a terminator is written, and an earlier
        // version of the check called it unsatisfiable.
        for m in [r"git add ($|[^a-z])", r"foo(x|$)", r"bar$"] {
            let problems = g("Bash", "command", Some(m), None).problems();
            assert!(problems.is_empty(), "{m:?} is fine, got {problems:?}");
        }
    }
}
