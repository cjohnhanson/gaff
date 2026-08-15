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
fn representatives(pattern: &str) -> Vec<String> {
    representatives_within(pattern, 0)
}

/// The most variants worth generating. An alternation multiplies, and
/// there is nothing to learn from thousands of them.
const CAP: usize = 32;

/// How deep a nest of groups is worth walking. Each level is one
/// recursive call, so an unbounded pattern overflowed the stack and
/// aborted `gaff check` with 134. No hand-written pattern comes close.
const MAX_DEPTH: usize = 32;

fn representatives_within(pattern: &str, depth: usize) -> Vec<String> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }

    let chars: Vec<char> = pattern.chars().collect();
    let mut out = vec![String::new()];
    let mut i = 0;
    while i < chars.len() && out.len() <= CAP {
        let c = chars[i];
        match c {
            // An anchor constrains the match further. It never removes
            // text from it, so the examples are unaffected.
            '^' | '$' => i += 1,
            '(' => {
                let Some(close) = closing(&chars, i) else {
                    return Vec::new();
                };
                let inner: String = chars[i + 1..close].iter().collect();
                let inner = inner.strip_prefix("?:").unwrap_or(&inner);
                let quantifier = chars.get(close + 1).copied();
                let branches = split_alternatives(inner);
                let mut next = Vec::new();
                for prefix in &out {
                    // An optional group yields a variant without it, so
                    // a claim has to hold whether or not it is present.
                    if matches!(quantifier, Some('?' | '*')) {
                        next.push(prefix.clone());
                    }
                    for branch in &branches {
                        for tail in representatives_within(branch, depth + 1) {
                            next.push(format!("{prefix}{tail}"));
                        }
                    }
                }
                out = next;
                i = close + 1;
                if matches!(quantifier, Some('?' | '*' | '+')) {
                    i += 1;
                }
            }
            '[' => {
                let Some(close) = chars[i..].iter().position(|c| *c == ']').map(|p| p + i) else {
                    return Vec::new();
                };
                let members = class_members(&chars[i + 1..close]);
                if members.is_empty() {
                    return Vec::new();
                }
                let quantifier = chars.get(close + 1).copied();
                let mut next = Vec::new();
                for prefix in &out {
                    if matches!(quantifier, Some('?' | '*')) {
                        next.push(prefix.clone());
                    }
                    // Every member, not one. Collapsing `[abc]` to `a`
                    // let an `unless` covering only `a` read as
                    // covering the class.
                    for member in &members {
                        next.push(format!("{prefix}{member}"));
                    }
                }
                out = next;
                i = close + 1;
                if matches!(quantifier, Some('?' | '*' | '+')) {
                    i += 1;
                }
            }
            '{' => {
                let Some(close) = chars[i..].iter().position(|c| *c == '}').map(|p| p + i) else {
                    return Vec::new();
                };
                // `{0…}` makes the atom optional, and the atom is
                // already in every variant. Give up rather than
                // unpick it.
                if chars[i + 1..close].first() == Some(&'0') {
                    return Vec::new();
                }
                i = close + 1;
            }
            _ => {
                let (atom, width) = atom_at(&chars, i);
                let Some(atom) = atom else {
                    return Vec::new();
                };
                let quantifier = chars.get(i + width).copied();
                let mut next = Vec::new();
                for prefix in &out {
                    if matches!(quantifier, Some('?' | '*')) {
                        next.push(prefix.clone());
                    }
                    next.push(format!("{prefix}{atom}"));
                }
                out = next;
                i += width;
                if matches!(quantifier, Some('?' | '*' | '+')) {
                    i += 1;
                }
            }
        }
    }
    if out.len() > CAP {
        return Vec::new();
    }
    out.retain(|s| !s.is_empty());
    out
}

/// The index of the `)` matching the `(` at `open`.
fn closing(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split on `|` at the top level of `body`.
fn split_alternatives(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                current.push(c);
                if let Some(n) = chars.next() {
                    current.push(n);
                }
            }
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            '|' if depth == 0 => out.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

/// One character the class can match.
///
/// A negated class needs a character the class does *not* list. Taking
/// the first character after the `^` returned exactly the one it
/// excludes, so a guard on `[^x]` was told an `unless` matching that
/// `x` covered it, and was switched off.
fn class_members(body: &[char]) -> Vec<char> {
    /// The most members worth listing from one class. The examples are
    /// there to test an `unless` against, not to enumerate a range.
    const PER_CLASS: usize = 4;

    let negated = body.first() == Some(&'^');
    let body = if negated { &body[1..] } else { body };
    let mut listed = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] == '\\' {
            match body.get(i + 1) {
                Some('d') => listed.extend('0'..='9'),
                Some('w') => {
                    listed.extend('a'..='z');
                    listed.extend('0'..='9');
                    listed.push('_');
                }
                Some('s') => listed.extend([' ', '\t']),
                // A negated set inside a class is a complement, and a
                // complement of a complement is not something this
                // walker models. Reading `\D` as the letter `D` made
                // `[^\D]` yield a letter, which that class can never
                // match.
                Some('D' | 'S' | 'W') | None => return Vec::new(),
                Some(n) => listed.push(*n),
            }
            i += 2;
            continue;
        }
        // A range, but only where the dash sits between two members.
        if i + 2 < body.len() && body[i + 1] == '-' {
            let (lo, hi) = (body[i], body[i + 2]);
            if lo <= hi {
                listed.extend(lo..=hi);
            }
            i += 3;
            continue;
        }
        listed.push(body[i]);
        i += 1;
    }
    if negated {
        // Ordinary characters the class does not list.
        return ('a'..='z')
            .chain('0'..='9')
            .filter(|c| !listed.contains(c))
            .take(PER_CLASS)
            .collect();
    }
    listed.truncate(PER_CLASS);
    listed
}

/// The text one atom contributes, and how many chars it spans.
fn atom_at(chars: &[char], i: usize) -> (Option<String>, usize) {
    match chars[i] {
        '\\' => match chars.get(i + 1) {
            Some('s') => (Some(" ".to_string()), 2),
            Some('d') => (Some("0".to_string()), 2),
            Some('w') => (Some("a".to_string()), 2),
            // A zero-width assertion adds nothing to the text.
            Some('b' | 'A' | 'z') => (Some(String::new()), 2),
            Some(n) if !n.is_alphanumeric() => (Some(n.to_string()), 2),
            _ => (None, 2),
        },
        '.' => (Some("x".to_string()), 1),
        // A bare quantifier or alternation here means the pattern is
        // shaped in a way this walker does not model.
        '|' | '?' | '*' | '+' => (None, 1),
        c => (Some(c.to_string()), 1),
    }
}

/// Whether `unless` exempts every string `matches` can catch.
///
/// Tested against example strings the pattern must be able to produce,
/// so a pattern with real structure is covered: `git\s+(add|stage)`
/// yields `git add` and `git stage`, and an `unless` of `git` matches
/// both, so the guard is decorative.
///
/// This is inference, so its verdict is a warning rather than a
/// refusal. An anchored `unless` is never treated as subsuming: `^rm`
/// exempts only a command that starts with `rm`, so it is strictly
/// narrower than the bare literal.
fn unless_subsumes(matches: &str, unless: &str) -> bool {
    if unless.contains('^') || unless.contains('$') || unless.contains("\\A") {
        return false;
    }
    let examples = representatives(matches);
    if examples.is_empty() {
        return false;
    }
    Regex::new(unless).is_ok_and(|re| examples.iter().all(|e| re.is_match(e)))
}

/// Why a pattern can never match, when that is decidable cheaply.
///
/// The regex crate exposes no emptiness oracle, so this covers the two
/// shapes an operator actually writes: an end anchor in the middle of a
/// pattern, and a word boundary between two word characters. Both read
/// as working rules and match nothing.
fn unsatisfiable(pattern: &str) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let multiline = pattern.contains("(?m");
    // Track character-class context. A `$` inside `[...]` is the
    // literal dollar sign, and reading it as an end anchor rejected an
    // ordinary guard on a shell variable and switched it off.
    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Count the run of backslashes: an odd run escapes, an even
        // run is that many literal backslashes. Looking back only two
        // characters read `\\\\\\$` as an unescaped anchor.
        let escaped = chars[..i].iter().rev().take_while(|c| **c == '\\').count() % 2 == 1;
        if !escaped {
            if c == '[' && !in_class {
                in_class = true;
                i += 1;
                continue;
            }
            if c == ']' && in_class {
                in_class = false;
                i += 1;
                continue;
            }
        }
        if in_class {
            i += 1;
            continue;
        }
        // `x$y`: nothing follows the end of the text.
        //
        // An end anchor closing an alternation branch is ordinary and
        // common — `($|[^a-z])` is exactly how a terminator is
        // written — so only a literal following the anchor counts.
        if c == '$'
            && !escaped
            && !multiline
            && let Some(next) = chars.get(i + 1)
            && !")|*?+{".contains(*next)
        {
            return Some("`$` is the end of the text and something follows it".to_string());
        }
        // `a\bb`: there is no word boundary between two word characters.
        // `(?m)` does not affect `\b`, so this check runs regardless.
        if c == 'b'
            && escaped
            && let Some(next) = chars.get(i + 1)
            && is_word(*next)
            && word_before(&chars, i - 1)
        {
            return Some(
                "`\\b` needs a word boundary and it sits between two word characters".to_string(),
            );
        }
        i += 1;
    }
    if !multiline
        && let Some(at) = pattern.find("\\z")
        && !pattern[at + 2..].is_empty()
        && !pattern[at + 2..].starts_with([')', '|'])
    {
        return Some("`\\z` is the end of the text and something follows it".to_string());
    }
    None
}

/// Whether the pattern text ending just before `at` can only produce a
/// word character.
///
/// `at` indexes the backslash of a `\b`. A literal is read directly. A
/// closing `]` means a character class, and a class of nothing but word
/// characters can only match one.
fn word_before(chars: &[char], at: usize) -> bool {
    let Some(prev) = at.checked_sub(1).and_then(|i| chars.get(i)) else {
        return false;
    };
    if *prev != ']' {
        return is_word(*prev);
    }
    let Some(open) = chars[..at - 1].iter().rposition(|c| *c == '[') else {
        return false;
    };
    let body = &chars[open + 1..at - 1];
    if body.is_empty() || body[0] == '^' {
        return false;
    }
    // A dash counts only as a range separator, between two members. A
    // leading or trailing one is a literal dash, which is not a word
    // character, so the boundary exists and the pattern is fine.
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        if c == '-' {
            let ranged = i > 0
                && i + 1 < body.len()
                && is_word(body[i - 1])
                && is_word(body[i + 1]);
            if !ranged {
                return false;
            }
        } else if !is_word(c) {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether a character is one a `\b` counts as part of a word.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
    /// Everything decided by inference lives here rather than in
    /// `problems`. A `problems` entry both fails `gaff check` and makes
    /// `first_refusal` skip the guard, so a wrong verdict there costs a
    /// silently disarmed control — the outcome this crate's own rules
    /// call worse than no guard at all. Three separate heuristics were
    /// each wrong about some legitimate pattern before this moved.
    /// `problems` now holds only what is decided from fact: an
    /// uncompilable regex, an unknown field, a tool that cannot send
    /// the field named.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        // An `unless` that covers everything `matches` catches makes
        // the guard decorative, and the probe corpus cannot see it:
        // `matches: 'git add'` with `unless: 'git'` exempts all of its
        // own hits, and no probe in the corpus has to contain `git`.
        // Compare the two patterns directly instead. This is a
        // heuristic over literal text, so it reports only the cases it
        // is certain of and stays quiet otherwise.
        if let (Some(m), Some(u)) = (&self.matches, &self.unless)
            && u.split('|')
                .filter(|a| !a.is_empty())
                .any(|alt| unless_subsumes(m, alt))
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

/// The guards gaff carries itself. No config declares them, and no
/// config removes them.
///
/// The boundary between a gaffed agent and a human shell is that every
/// agent command passes through `gaff hook` first. So gaff can make its
/// own privileged commands unrunnable from an agent, structurally: the
/// human's shell has no hook, the agent's has this one. A terminal check
/// on the command itself is a second line, not the first.
///
/// `gaff trust` grants a repo the right to run commands. `gaff allow`
/// grants an exception to a guard. Neither is the agent's to grant.
#[must_use]
pub fn builtin() -> Vec<Guard> {
    vec![Guard {
        name: "gaff-privileged".into(),
        tool: "Bash".into(),
        matches: Some(
            r"(^|[;&|(\n`]|\$\()[ \t]*(\S*/)?gaff[ \t]+(trust|allow)\b".into(),
        ),
        field: "command".into(),
        unless: None,
        message: "`gaff trust` and `gaff allow` grant rights an agent may not grant itself. The user runs them from a terminal: `!gaff allow <guard>` or `!gaff trust`.".into(),
    }]
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
        // Check the tool before the field. A guard that does not name
        // this tool has no business reading the payload, and looking
        // first meant every Bash guard reported a missing `command`
        // field on every Read call.
        .filter(|g| g.matches_tool(tool))
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
            matches: Some(r#"git((?:[ \t]|\\\r?\n)+-[^\s"';&|()<>]+((?:[ \t]|\\\r?\n)+[^\s"';&|()<>]+)?)*(?:[ \t]|\\\r?\n)+(add|stage)((?:[ \t]|\\\r?\n)+(?:"[^"]*"|'[^']*'|[^\s"';&|()<>]+))*?(?:[ \t]|\\\r?\n)+["']?(-[A-Za-z]*A[A-Za-z]*|--all|\.\.?/*\*?|:/\.?|:\(top\)|\*)["']?($|[^A-Za-z0-9_/.-])"#.into()),
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
            // A newline ends a command in shell, so the scan must stop
            // at one. A backslash continues the line, and then the
            // command genuinely spans it.
            "cd /x\ngit add -A",
            "git add \\\n-A",
            "git \\\n  add \\\n  -A",
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
            let warnings = g("Bash", "command", Some(m), Some(u)).warnings();
            assert!(
                warnings.iter().any(|p| p.contains("covers everything")),
                "matches {m:?} unless {u:?} should be caught, got {warnings:?}"
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
            let guard = g("Bash", "command", Some(m), Some(u));
            assert!(guard.problems().is_empty(), "{:?}", guard.problems());
            assert!(
                guard.warnings().is_empty(),
                "matches {m:?} unless {u:?} is legitimate, got {:?}",
                guard.warnings()
            );
        }
    }

    #[test]
    fn a_pattern_that_can_never_match_is_caught() {
        for m in ["x$y", r"a\bb", r"\Ax\z\Ay"] {
            let warnings = g("Bash", "command", Some(m), None).warnings();
            assert!(
                warnings.iter().any(|p| p.contains("can never match")),
                "{m:?} should be caught, got {warnings:?}"
            );
        }
    }

    #[test]
    fn an_end_anchor_closing_an_alternation_is_not_flagged() {
        // `($|[^a-z])` is how a terminator is written, and an earlier
        // version of the check called it unsatisfiable.
        for m in [r"git add ($|[^a-z])", r"foo(x|$)", r"bar$"] {
            let guard = g("Bash", "command", Some(m), None);
            assert!(guard.problems().is_empty(), "{:?}", guard.problems());
            assert!(
                guard.warnings().is_empty(),
                "{m:?} is fine, got {:?}",
                guard.warnings()
            );
        }
    }
}

#[cfg(test)]
mod validator_tests {
    use super::*;

    fn g(matches: Option<&str>, unless: Option<&str>) -> Guard {
        Guard {
            name: "g".into(),
            tool: "Bash".into(),
            matches: matches.map(Into::into),
            field: "command".into(),
            unless: unless.map(Into::into),
            message: "no".into(),
        }
    }

    #[test]
    fn an_anchored_unless_is_never_read_as_subsuming() {
        // `^rm` exempts only a command that starts with `rm`, so it is
        // narrower than the bare literal. Reading it as subsuming both
        // rejected a working guard and switched it off, which is the
        // worst pair of outcomes here.
        for (m, u) in [("rm", "^rm"), ("rm$", "^rm"), ("rm", "rm$")] {
            let guard = g(Some(m), Some(u));
            assert!(guard.problems().is_empty(), "{:?}", guard.problems());
            assert!(
                guard.warnings().is_empty(),
                "matches {m:?} unless {u:?} is a working guard, got {:?}",
                guard.warnings()
            );
        }
    }

    #[test]
    fn subsumption_is_found_through_pattern_structure() {
        // A real guard's `matches` is mostly structure, so a check that
        // only fired on two bare literals never fired in practice.
        for (m, u) in [
            (r"git\s+add", "git"),
            (r"rm\s+-rf", "rm"),
            ("rm[ ]-rf", "rm"),
            ("rm -rf", "rm.-rf"),
            ("rm -rf", "r[m]"),
            ("rm -rf", "rm|ls|cd"),
            (r"\.pem$", "pem"),
        ] {
            let warnings = g(Some(m), Some(u)).warnings();
            assert!(
                warnings.iter().any(|p| p.contains("covers everything")),
                "matches {m:?} unless {u:?} should be caught, got {warnings:?}"
            );
        }
    }

    #[test]
    fn a_narrower_unless_is_left_alone() {
        for (m, u) in [
            (r"git\s+add", "--dry-run"),
            (r"\.pem$", "example"),
            ("rm -rf", "--help"),
            ("rm -rf", "deploy-preview"),
        ] {
            let guard = g(Some(m), Some(u));
            assert!(guard.problems().is_empty(), "{:?}", guard.problems());
            assert!(
                guard.warnings().is_empty(),
                "matches {m:?} unless {u:?} is legitimate, got {:?}",
                guard.warnings()
            );
        }
    }

    #[test]
    fn a_dollar_inside_a_character_class_is_not_an_anchor() {
        // A guard on a shell variable is an ordinary thing to write,
        // and reading `[$]` as an end anchor rejected it and switched
        // it off.
        for m in ["rm -rf [$]HOME", "[$]x", r"\$5", r"\$[0-9]"] {
            let guard = g(Some(m), None);
            assert!(guard.problems().is_empty(), "{:?}", guard.problems());
            assert!(
                guard.warnings().is_empty(),
                "{m:?} is fine, got {:?}",
                guard.warnings()
            );
        }
    }

    #[test]
    fn a_word_boundary_after_a_word_class_is_caught() {
        // `[0-9]` can only match a word character, so there is no
        // boundary between it and the `x`.
        assert!(
            g(Some(r"[0-9]\bx"), None)
                .warnings()
                .iter()
                .any(|p| p.contains("can never match"))
        );
    }

    #[test]
    fn multiline_does_not_switch_off_the_word_boundary_check() {
        // `(?m)` changes what `$` means. It does nothing to `\b`, so it
        // must not be a blanket escape from the whole check.
        assert!(
            g(Some(r"(?m)a\bb"), None)
                .warnings()
                .iter()
                .any(|p| p.contains("can never match"))
        );
        // And it does excuse a mid-pattern `$`.
        assert!(g(Some("(?m)x$y"), None).warnings().is_empty());
    }
}

#[cfg(test)]
mod heuristic_tests {
    use super::*;

    fn g(matches: &str, unless: Option<&str>) -> Guard {
        Guard {
            name: "g".into(),
            tool: "Bash".into(),
            matches: Some(matches.into()),
            field: "command".into(),
            unless: unless.map(Into::into),
            message: "no".into(),
        }
    }

    /// A wrong verdict must never disarm the guard it misjudges.
    ///
    /// `problems` gates `first_refusal`; `warnings` does not. Every
    /// verdict reached by inference belongs in the second, so being
    /// wrong costs a noisy line rather than a silent hole.
    #[test]
    fn a_heuristic_verdict_never_reaches_problems() {
        for (m, u) in [
            ("rm", Some("^rm")),
            ("rm -rf [$]HOME", None),
            (r"git\s+add", Some("git")),
            ("x$y", None),
            (r"a\bb", None),
        ] {
            assert!(
                g(m, u).problems().is_empty(),
                "matches {m:?} unless {u:?} must not be a hard failure: {:?}",
                g(m, u).problems()
            );
        }
    }

    #[test]
    fn a_negated_class_yields_a_character_it_admits() {
        // Taking the first character after the `^` returned exactly the
        // one the class excludes, so a working guard was called
        // decorative.
        for (m, u) in [
            ("rm [^x]f", "rm xf"),
            ("deploy [^p]rod", "deploy prod"),
            ("x[^-a]z", "xaz"),
        ] {
            assert!(
                !unless_subsumes(m, u),
                "matches {m:?} unless {u:?} does not subsume"
            );
        }
    }

    #[test]
    fn an_escaped_class_member_is_not_a_backslash() {
        // `[\d]` matches a digit. No match of it holds a backslash.
        for m in [r"a[\d]b", r"a[\s]b", r"a[\w]b"] {
            assert!(!unless_subsumes(m, r"\\"), "{m:?} holds no backslash");
        }
        assert!(unless_subsumes(r"a[\d]b", "a"), "{:?}", representatives(r"a[\d]b"));
    }

    #[test]
    fn a_literal_dash_in_a_class_is_not_a_word_character() {
        // `x[a-]\bz` can produce `x-z`, and there is a boundary between
        // `-` and `z`, so the pattern is satisfiable.
        for m in [r"x[a-]\bz", r"x[0-9-]\bz", r"x[-a]\bz"] {
            assert!(
                unsatisfiable(m).is_none(),
                "{m:?} is satisfiable, got {:?}",
                unsatisfiable(m)
            );
        }
        // A class of nothing but word characters still has no boundary.
        assert!(unsatisfiable(r"x[a-z]\bz").is_some());
        assert!(unsatisfiable(r"[a-z_]\bx").is_some());
    }

    #[test]
    fn subsumption_covers_the_shapes_a_real_guard_has() {
        // Every one of these was skipped when a `|`, `?`, or `*`
        // anywhere in the pattern gave up — which is every real guard.
        for (m, u) in [
            (r"git\s+(add|stage)", "git"),
            (r"git\s+adds?", "git"),
            (r"git\s*add", "git"),
            (r"git(\s+-\S+)*\s+(add|stage)\s+(-A|--all|\.)", "git"),
        ] {
            assert!(
                unless_subsumes(m, u),
                "matches {m:?} unless {u:?} subsumes; examples {:?}",
                representatives(m)
            );
        }
    }

    #[test]
    fn a_narrower_unless_still_survives_the_wider_check() {
        for (m, u) in [
            (r"git\s+(add|stage)", "--dry-run"),
            (r"git\s+adds?", "stage"),
            (r"rm\s+-rf\s+/", "--dry-run"),
            (r"\.pem$", "example"),
        ] {
            assert!(
                !unless_subsumes(m, u),
                "matches {m:?} unless {u:?} does not subsume; examples {:?}",
                representatives(m)
            );
        }
    }

    #[test]
    fn an_escaped_dollar_behind_a_literal_backslash_is_not_an_anchor() {
        // Looking back only two characters read the run wrong.
        assert!(unsatisfiable(r"\\\$x").is_none());
        assert!(unsatisfiable(r"\$x").is_none());
        assert!(unsatisfiable("x$y").is_some());
    }

    #[test]
    fn the_shipped_pattern_survives_every_heuristic() {
        // The pattern gaff itself documents must not be reported as
        // broken by any of this.
        let doc = crate::docs::topic("configuration").unwrap();
        let mut shipped = Vec::new();
        for line in doc.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("matches: '") {
                shipped.push(rest.trim_end_matches('\'').replace("''", "'"));
            }
        }
        assert!(!shipped.is_empty(), "the docs show at least one guard");
        for pattern in shipped {
            let guard = g(&pattern, None);
            assert!(guard.problems().is_empty(), "{pattern}: {:?}", guard.problems());
            assert!(guard.warnings().is_empty(), "{pattern}: {:?}", guard.warnings());
        }
    }
}

#[cfg(test)]
mod expander_tests {
    use super::*;

    #[test]
    fn a_deeply_nested_pattern_does_not_overflow_the_stack() {
        // Each group level is one recursive call. Unbounded, a pattern
        // nested a few thousand deep aborted `gaff check` with 134,
        // which is neither 0 nor 1.
        for depth in [100usize, 3500, 8000] {
            let pattern = format!("{}x{}", "(".repeat(depth), ")".repeat(depth));
            let guard = Guard {
                name: "g".into(),
                tool: "Bash".into(),
                matches: Some(pattern),
                field: "command".into(),
                unless: Some("x".into()),
                message: "no".into(),
            };
            // Reaching either line at all is the assertion: an overflow
            // aborts the process.
            let _ = guard.problems();
            let _ = guard.warnings();
        }
    }

    #[test]
    fn a_class_contributes_every_member_it_can() {
        // Collapsing `[abc]` to `a` let an `unless` covering only `a`
        // read as covering the class.
        for (m, u) in [
            ("x[abc]y", "xay"),
            ("x[A-Z]y", "xAy"),
            ("x[0-9]y", "x0y"),
            (r"x[\d]y", "x0y"),
            ("x[-a]y", "x-y"),
        ] {
            assert!(
                !unless_subsumes(m, u),
                "matches {m:?} unless {u:?} covers one member, not the class; examples {:?}",
                representatives(m)
            );
        }
        // An unless covering every member still subsumes.
        assert!(unless_subsumes("x[abc]y", "x"));
    }

    #[test]
    fn a_complement_inside_a_class_is_not_modelled() {
        // `[^\D]` matches only digits. Reading `\D` as the letter `D`
        // produced a letter, which that class can never match.
        for m in [r"x[^\D]y", r"x[^\S]y", r"x[^\W]y"] {
            assert!(
                !unless_subsumes(m, "xay"),
                "{m:?} must not be modelled from a letter"
            );
        }
    }

    #[test]
    fn a_truncated_expansion_reports_nothing_rather_than_something_wrong() {
        // Past the cap the walker returns prefixes of real matches. A
        // report built on those could only ever be missing, never
        // wrong, and the empty result makes that explicit.
        let wide = "(a|b|c|d|e|f|g|h)(1|2|3|4|5)Z";
        assert!(
            !unless_subsumes(wide, "a"),
            "an unless matching one branch never subsumes"
        );
    }
}

#[cfg(test)]
mod builtin_tests {
    use super::*;

    #[test]
    fn the_privileged_commands_are_refused_from_an_agent() {
        // Every agent command passes through the hook, and a human
        // shell's does not. That boundary is what makes gaff's own
        // grants unrunnable from an agent, structurally.
        let guards = builtin();
        for cmd in [
            "gaff trust",
            "gaff allow no-mass-stage",
            "cd /x && gaff trust",
            "  gaff allow x",
            "/nix/store/abc/bin/gaff trust",
            "~/Projects/gaff/target/debug/gaff allow x",
            "echo hi; gaff trust",
            "true | gaff allow x",
            "(gaff trust)",
            "$(gaff allow x)",
            "`gaff trust`",
        ] {
            let value = |f: &str| (f == "command").then(|| cmd.to_string());
            assert!(
                first_refusal(&guards, "Bash", &value).is_some(),
                "{cmd:?} must be refused"
            );
        }
    }

    #[test]
    fn ordinary_gaff_commands_pass() {
        let guards = builtin();
        for cmd in [
            "gaff status",
            "gaff remind x --after 5",
            "gaff remind --clear --id x",
            "gaff check",
            "gaff doctor",
            "gaff log",
            "gaff docs configuration",
            "gaff init --git",
            "echo 'gaff trust is for humans'",
            "grep -n 'gaff allow' src/cli.rs",
            "cat gaff-trust-notes.md",
            "gaffer trust",
        ] {
            let value = |f: &str| (f == "command").then(|| cmd.to_string());
            assert!(
                first_refusal(&guards, "Bash", &value).is_none(),
                "{cmd:?} must pass"
            );
        }
    }

    #[test]
    fn a_builtin_guard_is_valid_by_construction() {
        for g in builtin() {
            assert!(g.problems().is_empty(), "{}: {:?}", g.name, g.problems());
            assert!(g.warnings().is_empty(), "{}: {:?}", g.name, g.warnings());
        }
    }
}
