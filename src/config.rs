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

#[derive(Debug, Deserialize)]
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

/// A prime section: a markdown file under `.gaff/`. gaff injects the
/// whole file at `SessionStart`. gaff injects it again on its refresh
/// cadence.
///
/// Sections and reminders share one namespace, because the pending state
/// and the cursor state use the name as the key. A name must be unique
/// across both.
#[derive(Debug, Deserialize)]
pub struct Section {
    pub name: String,
    /// The path to the section body, relative to `.gaff/`.
    pub file: String,
    #[serde(default)]
    pub refresh: Every,
}

/// A cadence: fire every N counted events of the given unit.
#[derive(Debug, Default, Deserialize)]
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
