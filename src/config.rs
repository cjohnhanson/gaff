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
    #[serde(default)]
    pub transitions: Transitions,
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
            transitions: Transitions::default(),
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

#[must_use]
pub fn load(cwd: &Path) -> Loaded {
    let path = cwd.join(CONFIG_PATH);
    let Ok(bytes) = std::fs::read_to_string(&path) else {
        return Loaded::Absent;
    };
    match serde_yml::from_str::<Config>(&bytes) {
        Ok(cfg) => Loaded::Ok(cfg),
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
        assert!(cfg.transitions.agent_may_set("focus"));
        assert!(!cfg.transitions.agent_may_set("quiet"), "human only");
    }
}
