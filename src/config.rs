//! The repo-level config: `.gaff/gaff.yml`. It holds data only.
//!
//! gaff executes nothing that a repo declares. A repo declares the
//! reminder text and the cadences. Anything that runs code lives in the
//! user-scoped config. Version 0 has no such config.
//!
//! A malformed config degrades loudly, and it never blocks. The caller
//! prints a warning on stderr, writes a marker, and continues without
//! reminders.

use std::path::Path;

use serde::Deserialize;

pub const CONFIG_PATH: &str = ".gaff/gaff.yml";
const DEFAULT_MAX_INJECT_BYTES: usize = 4096;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub reminders: Vec<Reminder>,
    /// The prime sections. gaff injects each file at session start. Each
    /// section refreshes mid-session on its own cadence.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// The hard cap on the bytes injected per flush. The cap includes
    /// the truncation marker.
    #[serde(default = "default_max_inject_bytes")]
    pub max_inject_bytes: usize,
    /// The named overlays. A profile selects which entries are active
    /// and may override their cadences.
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, Profile>,
    /// The profile that applies when nothing else selects one.
    #[serde(default)]
    pub default_profile: Option<String>,
    /// Which profile switches an agent may make on its own session.
    /// An absent value and an empty list mean different things. An
    /// empty `agent_may_set` says the agent may set nothing, and a
    /// repo must not be able to overrule that by looking unset.
    #[serde(default)]
    pub transitions: Option<Transitions>,
    /// The git-hook entries. gaff writes the hook scripts, and they
    /// call back into gaff, so one config covers both domains.
    #[serde(default)]
    pub git: Vec<crate::githook::GitHook>,
    /// The workflows to generate. gaff cannot run a GitHub event, so
    /// this domain is generated and checked, never executed.
    #[serde(default)]
    pub github: Vec<crate::ghworkflow::Workflow>,
    /// The tool calls to refuse. This is the only feature that blocks.
    #[serde(default)]
    pub guards: Vec<crate::guard::Guard>,
}

/// A profile: a named overlay on the reminders and the sections.
///
/// A profile never adds an entry. It selects from the entries the base
/// config already declares, and it may override their cadences. That
/// keeps one namespace and one place to read what a repo can inject.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Keep only these entries. `None` keeps every entry.
    #[serde(default)]
    pub only: Option<Vec<String>>,
    /// Drop these entries. `disable` applies after `only`.
    #[serde(default)]
    pub disable: Vec<String>,
    /// Cadence overrides, keyed by the entry name.
    #[serde(default)]
    pub cadence: std::collections::BTreeMap<String, Every>,
    /// The byte cap under this profile. `None` keeps the base cap.
    #[serde(default)]
    pub max_inject_bytes: Option<usize>,
}

/// The transition policy for profile switches.
///
/// Profiles are advisory: gaff blocks nothing, and an agent that can
/// write files can edit `.gaff/gaff.yml` anyway. The policy states
/// intent and refuses the agent-facing path, so a switch an operator
/// did not sanction is at least not a supported one.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transitions {
    /// The profiles an agent may select for itself. A profile absent
    /// from this list is human-only.
    #[serde(default)]
    pub agent_may_set: Vec<String>,
}

impl Transitions {
    /// Whether an agent may select `name`.
    #[must_use]
    pub fn agent_may_set(&self, name: &str) -> bool {
        self.agent_may_set.iter().any(|p| p == name)
    }
}

impl Config {
    /// Apply a profile overlay and return the effective config.
    ///
    /// An unknown name applies nothing and warns. A typo must never
    /// silently empty the config, because a silent empty config looks
    /// exactly like a working one.
    #[must_use]
    pub fn with_profile(&self, name: Option<&str>) -> Self {
        let mut out = self.clone();
        let Some(name) = name else {
            return out;
        };
        let Some(profile) = self.profiles.get(name) else {
            eprintln!("gaff: unknown profile `{name}`. Using the base config.");
            return out;
        };
        let keep = |n: &str| {
            profile
                .only
                .as_ref()
                .is_none_or(|only| only.iter().any(|k| k == n))
                && !profile.disable.iter().any(|d| d == n)
        };
        out.reminders.retain(|r| keep(&r.name));
        out.sections.retain(|s| keep(&s.name));
        for r in &mut out.reminders {
            if let Some(c) = profile.cadence.get(&r.name) {
                r.every = c.clone();
            }
        }
        for s in &mut out.sections {
            if let Some(c) = profile.cadence.get(&s.name) {
                s.refresh = c.clone();
            }
        }
        if let Some(cap) = profile.max_inject_bytes {
            out.max_inject_bytes = cap;
        }
        out
    }
}

/// Resolve the active profile name. The resolution path is the flag,
/// then the environment, then the session state, then `.gaff/profile`,
/// then the config default. The first hit wins.
#[must_use]
pub fn resolve_profile(
    flag: Option<&str>,
    env: Option<&str>,
    session: Option<&str>,
    gaff_dir: &Path,
    config: &Config,
) -> Option<String> {
    let from_file = || {
        std::fs::read_to_string(gaff_dir.join("profile"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    flag.map(ToString::to_string)
        .or_else(|| env.map(ToString::to_string))
        .or_else(|| session.map(ToString::to_string))
        .or_else(from_file)
        .or_else(|| config.default_profile.clone())
}

/// A derived `Default` would set the cap to zero. That silently
/// suppresses every flush on the no-config path. This manual
/// implementation keeps the serde default and the absent-config default
/// the same.
impl Default for Config {
    fn default() -> Self {
        Self {
            reminders: Vec::new(),
            sections: Vec::new(),
            max_inject_bytes: DEFAULT_MAX_INJECT_BYTES,
            profiles: std::collections::BTreeMap::new(),
            default_profile: None,
            transitions: None,
            git: Vec::new(),
            github: Vec::new(),
            guards: Vec::new(),
        }
    }
}

const fn default_max_inject_bytes() -> usize {
    DEFAULT_MAX_INJECT_BYTES
}

/// Resolve a section file path under `gaff_dir` and confine it there.
///
/// A section file is repo data. It must not read outside `.gaff/`. This
/// rejects an absolute path and any `..` that escapes the directory,
/// which stops a committed config from reading a file into the model's
/// context. The check is lexical, so it needs no file on disk and works
/// the same in `gaff check` and at read time.
///
/// # Errors
/// Returns the offending path string when the file is absolute or leaves
/// `gaff_dir`.
pub fn confine_section_path(gaff_dir: &Path, file: &str) -> Result<std::path::PathBuf, String> {
    let candidate = Path::new(file);
    if candidate.is_absolute() {
        return Err(file.to_string());
    }
    let mut depth: i32 = 0;
    for component in candidate.components() {
        match component {
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(file.to_string());
                }
            }
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(file.to_string());
            }
        }
    }
    Ok(gaff_dir.join(candidate))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reminder {
    pub name: String,
    pub every: Every,
    pub text: String,
}

/// A prime section: a markdown file under `.gaff/`. gaff injects the
/// whole file at `SessionStart`. gaff injects it again on its refresh
/// cadence.
///
/// Sections and reminders share one namespace, because the pending state
/// and the cursor state use the name as the key. A name must be unique
/// across both.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub name: String,
    /// The path to the section body, relative to `.gaff/`.
    pub file: String,
    #[serde(default)]
    pub refresh: Every,
}

/// A cadence: fire every N counted events of the given unit.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Every {
    #[serde(default)]
    pub tool_calls: Option<u64>,
    #[serde(default)]
    pub prompts: Option<u64>,
}

/// The outcome of a config load. `Broken` carries the parse error, so
/// the caller can print a warning. A broken config never does more than
/// warn.
#[derive(Debug)]
pub enum Loaded {
    Absent,
    Ok(Config),
    Broken(String),
}

/// The user-scoped data config. It holds the same keys as a repo
/// config, and it never holds handlers.
///
/// Handlers stay in their own file, so the security boundary stays
/// trivially checkable: a command can only come from `handlers.yml`.
#[must_use]
pub fn user_config_path() -> Option<std::path::PathBuf> {
    crate::handler::config_dir().map(|d| d.join("gaff.yml"))
}

/// Load the user config, then lay the repo config over it.
///
/// A person works in many repos and wants some reminders everywhere.
/// The repo is the more specific scope, so a repo entry wins over a
/// user entry of the same name.
#[must_use]
pub fn load_layered(cwd: &Path) -> Loaded {
    let user = match user_config_path() {
        Some(p) => match std::fs::read_to_string(&p) {
            Ok(text) => match serde_yml::from_str::<Config>(&text) {
                Ok(cfg) => Some(cfg),
                Err(e) => {
                    return Loaded::Broken(format!("{}: {e}", p.display()));
                }
            },
            // An absent file is the ordinary case. Any other error means
            // the file exists and gaff cannot read it, and treating that
            // as "no guards" would silently disarm every refusal.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Loaded::Broken(format!("{}: {e}", p.display()));
            }
        },
        None => None,
    };
    match (user, load(cwd)) {
        (None, repo) => repo,
        (Some(user), Loaded::Absent) => Loaded::Ok(user),
        (Some(user), Loaded::Ok(repo)) => Loaded::Ok(user.overlaid_with(repo)),
        // A repo that cannot be parsed contributes nothing. It must
        // not delete what the user declared, because the user's guards
        // are refusals and a repo could otherwise switch them all off
        // with one bad line.
        (Some(user), Loaded::Broken(err)) => {
            eprintln!("gaff: {CONFIG_PATH} is not valid: {err}. Using the user config alone.");
            Loaded::Ok(user)
        }
    }
}

impl Config {
    /// Lay `repo` over `self`, where `self` is the user config.
    ///
    /// Reminders, sections, and profiles merge by name, and the repo
    /// entry replaces the user entry it shadows. A scalar takes the
    /// repo value when the repo sets one.
    ///
    /// `transitions` is the exception: the user value wins whenever the
    /// user sets one. That field says which profiles an agent may grant
    /// itself, and a repo must not widen it.
    #[must_use]
    pub fn overlaid_with(mut self, repo: Self) -> Self {
        let user_set_transitions = self.transitions.is_some();
        // A repo may not redefine a profile the user declared. The user
        // sanctions a profile by name, and a repo that could rewrite
        // that name would decide what the sanctioned profile does.
        let user_named: Vec<String> = self.profiles.keys().cloned().collect();

        self.reminders
            .retain(|u| !repo.reminders.iter().any(|r| r.name == u.name));
        self.reminders.extend(repo.reminders);
        self.sections
            .retain(|u| !repo.sections.iter().any(|r| r.name == u.name));
        self.sections.extend(repo.sections);
        for (name, profile) in repo.profiles {
            if user_named.contains(&name) {
                eprintln!(
                    "gaff: the repo redefines the profile `{name}`, which the user declared. Keeping the user's."
                );
                continue;
            }
            self.profiles.insert(name, profile);
        }
        self.git.retain(|u| !repo.git.iter().any(|r| r.name == u.name));
        self.git.extend(repo.git);
        self.github
            .retain(|u| !repo.github.iter().any(|r| r.name == u.name));
        self.github.extend(repo.github);
        // A guard comes from the user config only. A repo cannot add
        // one, and a repo cannot remove one.
        //
        // Guards are the single blocking feature, and a repo is
        // untrusted content. A repo-declared guard is a cloned
        // repository deciding which tool calls its reader may make, and
        // a repo declaring `tool: '.*'` would refuse every call. The
        // repo's guards are dropped here rather than merged.
        if !repo.guards.is_empty() {
            eprintln!(
                "gaff: the repo config declares {} guard(s). A repo may not declare a guard, so they are ignored. Guards belong in $HOME/.config/gaff/gaff.yml.",
                repo.guards.len()
            );
        }

        if repo.max_inject_bytes != DEFAULT_MAX_INJECT_BYTES {
            self.max_inject_bytes = repo.max_inject_bytes;
        }
        if repo.default_profile.is_some() {
            self.default_profile = repo.default_profile;
        }
        if !user_set_transitions {
            self.transitions = repo.transitions;
        }
        self
    }
}

#[must_use]
pub fn load(cwd: &Path) -> Loaded {
    let path = cwd.join(CONFIG_PATH);
    let Ok(bytes) = std::fs::read_to_string(&path) else {
        return Loaded::Absent;
    };
    match serde_yml::from_str::<Config>(&bytes) {
        Ok(cfg) => Loaded::Ok(cfg),
        // Name this one, because the generic serde message does not say
        // where handlers belong, and the reader has to know that a repo
        // may never declare a command.
        Err(e) if e.to_string().contains("unknown field `handlers`") => Loaded::Broken(
            "a repo may not declare handlers. They are user-scoped, in \
             $HOME/.config/gaff/handlers.yml, because a repo-declared command \
             would run on clone."
                .to_string(),
        ),
        Err(e) => Loaded::Broken(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reminders_and_cap() {
        let cfg: Config = serde_yml::from_str(
            "max_inject_bytes: 64\nreminders:\n  - name: a\n    every:\n      tool_calls: 3\n    text: hi\n",
        )
        .unwrap();
        assert_eq!(cfg.max_inject_bytes, 64);
        assert_eq!(cfg.reminders.len(), 1);
        assert_eq!(cfg.reminders[0].every.tool_calls, Some(3));
        assert_eq!(cfg.reminders[0].every.prompts, None);
    }

    #[test]
    fn cap_defaults_when_absent() {
        let cfg: Config = serde_yml::from_str("reminders: []\n").unwrap();
        assert_eq!(cfg.max_inject_bytes, DEFAULT_MAX_INJECT_BYTES);
    }

    #[test]
    fn broken_yaml_reports_broken() {
        let dir = std::env::temp_dir().join(format!("gaff-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".gaff")).unwrap();
        std::fs::write(dir.join(CONFIG_PATH), "reminders: [oops\n").unwrap();
        assert!(matches!(load(&dir), Loaded::Broken(_)));
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    fn base() -> Config {
        serde_yml::from_str(
            "reminders:\n  - name: a\n    every: {tool_calls: 5}\n    text: A\n  - name: b\n    every: {tool_calls: 7}\n    text: B\nsections:\n  - name: s\n    file: s.md\n    refresh: {prompts: 3}\nprofiles:\n  focus:\n    only: [a]\n    cadence:\n      a: {tool_calls: 2}\n  quiet:\n    disable: [a, b, s]\n    max_inject_bytes: 100\ndefault_profile: focus\ntransitions:\n  agent_may_set: [focus]\n",
        )
        .expect("the fixture config must parse")
    }

    #[test]
    fn only_selects_and_cadence_overrides() {
        let cfg = base().with_profile(Some("focus"));
        assert_eq!(cfg.reminders.len(), 1, "only: [a] keeps one reminder");
        assert_eq!(cfg.reminders[0].name, "a");
        assert_eq!(
            cfg.reminders[0].every.tool_calls,
            Some(2),
            "the profile overrides the cadence"
        );
        assert!(cfg.sections.is_empty(), "only: [a] drops the section");
    }

    #[test]
    fn disable_drops_entries_and_overrides_the_cap() {
        let cfg = base().with_profile(Some("quiet"));
        assert!(cfg.reminders.is_empty());
        assert!(cfg.sections.is_empty());
        assert_eq!(cfg.max_inject_bytes, 100);
    }

    #[test]
    fn an_unknown_profile_keeps_the_base_config() {
        // A typo must never silently empty the config.
        let cfg = base().with_profile(Some("nope"));
        assert_eq!(cfg.reminders.len(), 2);
        assert_eq!(cfg.sections.len(), 1);
    }

    #[test]
    fn resolution_order_prefers_the_earlier_source() {
        let cfg = base();
        let dir = std::env::temp_dir().join(format!("gaff-prof-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile"), "fromfile\n").unwrap();

        assert_eq!(
            resolve_profile(Some("flag"), Some("env"), Some("sess"), &dir, &cfg).as_deref(),
            Some("flag")
        );
        assert_eq!(
            resolve_profile(None, Some("env"), Some("sess"), &dir, &cfg).as_deref(),
            Some("env")
        );
        assert_eq!(
            resolve_profile(None, None, Some("sess"), &dir, &cfg).as_deref(),
            Some("sess")
        );
        assert_eq!(
            resolve_profile(None, None, None, &dir, &cfg).as_deref(),
            Some("fromfile")
        );
        std::fs::remove_file(dir.join("profile")).unwrap();
        assert_eq!(
            resolve_profile(None, None, None, &dir, &cfg).as_deref(),
            Some("focus"),
            "the config default is the last resort"
        );
    }

    #[test]
    fn the_transition_policy_names_the_agent_settable_profiles() {
        let cfg = base();
        assert!(cfg.transitions.clone().unwrap_or_default().agent_may_set("focus"));
        assert!(!cfg.transitions.unwrap_or_default().agent_may_set("quiet"), "human only");
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    fn cfg(yaml: &str) -> Config {
        serde_yml::from_str(yaml).expect("the fixture must parse")
    }

    #[test]
    fn a_user_reminder_applies_where_a_repo_declares_none() {
        let user = cfg("reminders:\n  - name: global\n    every: {tool_calls: 9}\n    text: G\n");
        let repo = cfg("reminders: []\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.reminders.len(), 1);
        assert_eq!(merged.reminders[0].name, "global");
    }

    #[test]
    fn a_repo_entry_replaces_the_user_entry_it_shadows() {
        // The repo is the more specific scope, so it wins the name.
        let user = cfg("reminders:\n  - name: same\n    every: {tool_calls: 1}\n    text: FROM_USER\n  - name: keep\n    every: {tool_calls: 2}\n    text: K\n");
        let repo = cfg("reminders:\n  - name: same\n    every: {tool_calls: 5}\n    text: FROM_REPO\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.reminders.len(), 2, "the unshadowed user entry survives");
        let same = merged.reminders.iter().find(|r| r.name == "same").unwrap();
        assert_eq!(same.text, "FROM_REPO");
        assert_eq!(same.every.tool_calls, Some(5));
        assert!(merged.reminders.iter().any(|r| r.name == "keep"));
    }

    #[test]
    fn a_repo_cannot_widen_what_an_agent_may_grant_itself() {
        // transitions is the one field the repo does not win. A repo
        // that could widen it would let an agent switch to any profile.
        let user = cfg("transitions:\n  agent_may_set: [safe]\n");
        let repo = cfg("transitions:\n  agent_may_set: [safe, dangerous]\n");
        let merged = user.overlaid_with(repo);
        assert!(merged.transitions.clone().unwrap_or_default().agent_may_set("safe"));
        assert!(
            !merged.transitions.unwrap_or_default().agent_may_set("dangerous"),
            "a repo must not widen the agent's own permissions"
        );
    }

    #[test]
    fn a_repo_sets_the_transitions_when_the_user_states_none() {
        let user = cfg("reminders: []\n");
        let repo = cfg("transitions:\n  agent_may_set: [focus]\n");
        let merged = user.overlaid_with(repo);
        assert!(merged.transitions.unwrap_or_default().agent_may_set("focus"));
    }

    #[test]
    fn the_repo_wins_the_scalars_it_sets() {
        let user = cfg("max_inject_bytes: 100\ndefault_profile: userdef\n");
        let repo = cfg("max_inject_bytes: 200\ndefault_profile: repodef\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.max_inject_bytes, 200);
        assert_eq!(merged.default_profile.as_deref(), Some("repodef"));
    }

    #[test]
    fn an_unset_repo_scalar_leaves_the_user_value() {
        let user = cfg("max_inject_bytes: 100\ndefault_profile: userdef\n");
        let repo = cfg("reminders: []\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.max_inject_bytes, 100, "the repo set no cap");
        assert_eq!(merged.default_profile.as_deref(), Some("userdef"));
    }
}

#[cfg(test)]
mod guard_layering_tests {
    use super::*;

    fn user_with_guard() -> Config {
        serde_yml::from_str(
            "guards:\n  - name: no-mass-stage\n    tool: Bash\n    matches: 'git add -A'\n    message: Stage by name.\n",
        )
        .expect("the user fixture must parse")
    }

    #[test]
    fn a_repo_cannot_declare_a_guard() {
        // A cloned repo is untrusted content. A repo-declared guard
        // would let it refuse any tool call its reader makes.
        let repo: Config = serde_yml::from_str(
            "guards:\n  - name: repo-owns-you\n    tool: '.*'\n    message: blocked\n",
        )
        .unwrap();
        let merged = user_with_guard().overlaid_with(repo);
        assert_eq!(merged.guards.len(), 1, "only the user guard survives");
        assert_eq!(merged.guards[0].name, "no-mass-stage");
    }

    #[test]
    fn a_repo_cannot_remove_a_user_guard() {
        let repo: Config = serde_yml::from_str("guards: []\n").unwrap();
        let merged = user_with_guard().overlaid_with(repo);
        assert_eq!(merged.guards.len(), 1);
    }

    #[test]
    fn a_repo_cannot_shadow_a_user_guard_by_name() {
        // A same-name guard with a pattern that matches nothing would
        // otherwise defuse the user's rule.
        let repo: Config = serde_yml::from_str(
            "guards:\n  - name: no-mass-stage\n    tool: Bash\n    matches: 'zzz'\n    message: neutered\n",
        )
        .unwrap();
        let merged = user_with_guard().overlaid_with(repo);
        assert_eq!(merged.guards.len(), 1);
        assert_eq!(merged.guards[0].matches.as_deref(), Some("git add -A"));
    }
}

#[cfg(test)]
mod transition_layering_tests {
    use super::*;

    fn cfg(y: &str) -> Config {
        serde_yml::from_str(y).expect("fixture must parse")
    }

    #[test]
    fn an_explicit_empty_list_means_the_agent_may_set_nothing() {
        // Absent and empty are different. A repo must not be able to
        // overrule "nothing" by looking unset.
        let user = cfg("profiles: {safe: {}}\ntransitions: {agent_may_set: []}\n");
        let repo = cfg("profiles: {wide: {}}\ntransitions: {agent_may_set: [wide, safe]}\n");
        let merged = user.overlaid_with(repo);
        let t = merged.transitions.unwrap_or_default();
        assert!(!t.agent_may_set("wide"), "a repo may not widen the list");
        assert!(!t.agent_may_set("safe"), "the user allowed nothing");
    }

    #[test]
    fn a_repo_cannot_redefine_a_profile_the_user_declared() {
        // The user sanctions a profile by name. A repo that rewrote
        // that name would decide what the sanctioned profile does.
        let user = cfg("reminders: [{name: safety, every: {tool_calls: 1}, text: KEEP}]\nprofiles: {safe: {}}\ntransitions: {agent_may_set: [safe]}\n");
        let repo = cfg("profiles: {safe: {only: []}}\n");
        let merged = user.overlaid_with(repo);
        let effective = merged.with_profile(Some("safe"));
        assert_eq!(
            effective.reminders.len(),
            1,
            "the repo must not empty the user's profile"
        );
        assert_eq!(effective.reminders[0].text, "KEEP");
    }

    #[test]
    fn a_repo_may_still_add_a_profile_of_its_own() {
        let user = cfg("profiles: {safe: {}}\n");
        let repo = cfg("profiles: {repoonly: {}}\n");
        let merged = user.overlaid_with(repo);
        assert!(merged.profiles.contains_key("safe"));
        assert!(merged.profiles.contains_key("repoonly"));
    }
}
