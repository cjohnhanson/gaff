//! Repo-level configuration: `.gaff/gaff.yml`, data only.
//!
//! Nothing in this file is executable. A repo declares reminder text and
//! cadences; anything that runs code lives in user-scoped config (none in
//! v0). A malformed config degrades loudly and never blocks: the caller
//! warns on stderr, drops a marker, and continues without reminders.

use std::path::Path;

use serde::Deserialize;

pub const CONFIG_PATH: &str = ".gaff/gaff.yml";
const DEFAULT_MAX_INJECT_BYTES: usize = 4096;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub reminders: Vec<Reminder>,
    /// Prime sections: files injected at session start and refreshable
    /// mid-session on their own cadences.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Hard cap on the bytes injected per flush, truncation marker included.
    #[serde(default = "default_max_inject_bytes")]
    pub max_inject_bytes: usize,
}

/// The derived `Default` would zero the cap and silently suppress every
/// flush on the no-config path — the manual impl keeps the serde default
/// and the absent-config default identical.
impl Default for Config {
    fn default() -> Self {
        Self {
            reminders: Vec::new(),
            sections: Vec::new(),
            max_inject_bytes: DEFAULT_MAX_INJECT_BYTES,
        }
    }
}

const fn default_max_inject_bytes() -> usize {
    DEFAULT_MAX_INJECT_BYTES
}

#[derive(Debug, Deserialize)]
pub struct Reminder {
    pub name: String,
    pub every: Every,
    pub text: String,
}

/// A prime section: a markdown file under `.gaff/`, injected in full at
/// `SessionStart` and re-injected on its refresh cadence.
///
/// Names share a namespace with reminders (pending/cursor state
/// is keyed by name) and must be unique across both.
#[derive(Debug, Deserialize)]
pub struct Section {
    pub name: String,
    /// Path to the section body, relative to `.gaff/`.
    pub file: String,
    #[serde(default)]
    pub refresh: Every,
}

/// Cadence spec: fire every N counted events of the given unit.
#[derive(Debug, Default, Deserialize)]
pub struct Every {
    #[serde(default)]
    pub tool_calls: Option<u64>,
    #[serde(default)]
    pub prompts: Option<u64>,
}

/// Outcome of a config load. `Broken` carries the parse error so the
/// caller can warn; it must never escalate past a warning.
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
