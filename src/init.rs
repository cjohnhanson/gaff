//! `gaff init` registers gaff as a handler in the harness's own hook
//! config. The write is atomic and idempotent. `gaff init --uninstall`
//! removes exactly what `gaff init` added, and nothing else.
//!
//! gaff owns no dispatch. This module writes ordinary entries into
//! `.claude/settings.local.json`, a local file that git ignores. The
//! entries sit beside whatever else the file holds, and gaff keeps every
//! unknown key. The rewrite writes a temporary file and renames it, so a
//! crash never leaves a truncated file.

use std::path::Path;

use serde_json::{json, Map, Value};

/// The default settings path. `gaff init --host <name>` uses the named
/// adapter's path instead.
pub const SETTINGS_PATH: &str = crate::adapter::CLAUDE_CODE.settings_path;

/// The events that gaff needs: the prime and flush points, plus the
/// counted events.
pub const HOOK_EVENTS: &[&str] = crate::adapter::CLAUDE_CODE_EVENTS;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Changed,
    Unchanged,
}

/// Install the hook entries for `command` into the settings file under
/// `root`. The default command is `gaff hook`. This creates the file
/// when the file is absent.
pub fn install(root: &Path, command: &str) -> std::io::Result<Outcome> {
    install_for(&crate::adapter::CLAUDE_CODE, root, command)
}

/// Install the hook entries for one adapter. The adapter owns the
/// settings path and the event names.
pub fn install_for(
    adapter: &crate::adapter::Adapter,
    root: &Path,
    command: &str,
) -> std::io::Result<Outcome> {
    edit_settings(adapter.settings_path, root, |settings| {
        let hooks = hooks_map(settings);
        let mut changed = false;
        for event in adapter.hook_events {
            let entries = hooks
                .entry(event.to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Value::Array(entries) = entries else {
                continue; // An unknown shape: leave the user's config alone.
            };
            if !entries.iter().any(|e| has_command(e, command)) {
                entries.push(json!({"hooks": [{"type": "command", "command": command}]}));
                changed = true;
            }
        }
        changed
    })
}

/// Remove every hook entry whose command matches `command`. This also
/// drops an empty event array and an empty `hooks` map.
pub fn uninstall(root: &Path, command: &str) -> std::io::Result<Outcome> {
    uninstall_for(&crate::adapter::CLAUDE_CODE, root, command)
}

/// Remove the hook entries for one adapter.
pub fn uninstall_for(
    adapter: &crate::adapter::Adapter,
    root: &Path,
    command: &str,
) -> std::io::Result<Outcome> {
    edit_settings(adapter.settings_path, root, |settings| {
        let hooks = hooks_map(settings);
        let mut changed = false;
        hooks.retain(|_, entries| {
            if let Value::Array(list) = entries {
                let before = list.len();
                list.retain(|e| !has_command(e, command));
                changed |= list.len() != before;
                !list.is_empty()
            } else {
                true
            }
        });
        if hooks.is_empty() {
            settings.remove("hooks");
        }
        changed
    })
}

/// The `hooks` object.
///
/// `edit_settings` refuses a file whose `hooks` key holds anything
/// else, so the key is either absent or an object by the time this
/// runs.
fn hooks_map(settings: &mut Map<String, Value>) -> &mut Map<String, Value> {
    settings
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("edit_settings refuses a non-object hooks key")
}

/// True when a matcher-group entry holds a hook with exactly this
/// command.
fn has_command(entry: &Value, command: &str) -> bool {
    entry["hooks"]
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(|h| h["command"] == command))
}

/// Load the settings, mutate them, write a temporary file, and rename
/// it. The mutator returns whether anything changed. gaff does not
/// rewrite an unchanged file.
fn edit_settings(
    settings_path: &str,
    root: &Path,
    mutate: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> std::io::Result<Outcome> {
    let declared = root.join(settings_path);
    // A dotfile manager installs this file as a link into a managed
    // store. Renaming over it severs the link, leaves the store file
    // untouched, and reports success. `read_user_config_file` goes out
    // of its way to support that layout; this path has to match.
    let path = std::fs::canonicalize(&declared).unwrap_or(declared);
    let mut settings: Map<String, Value> = match std::fs::read_to_string(&path) {
        Ok(bytes) => serde_json::from_str(&bytes).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{}: not valid JSON ({e}). Refusing to rewrite the file.",
                    path.display()
                ),
            )
        })?,
        Err(_) => Map::new(),
    };

    // Replacing a non-object `hooks` value dropped whatever the user
    // had there, silently and at exit 0, which contradicts the promise
    // that gaff keeps every key it does not own.
    if settings.get("hooks").is_some_and(|h| !h.is_object()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{}: the `hooks` key is not an object. Refusing to rewrite the file, because that would drop what is there.",
                path.display()
            ),
        ));
    }

    if !mutate(&mut settings) {
        return Ok(Outcome::Unchanged);
    }

    std::fs::create_dir_all(path.parent().expect("settings path has a parent"))?;
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(settings)).expect("maps serialize")
    );
    let tmp = path.with_extension("json.gaff-tmp");
    std::fs::write(&tmp, rendered)?;
    std::fs::rename(&tmp, &path)?;
    Ok(Outcome::Changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("gaff-init-test-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn install_is_idempotent() {
        let root = temp_root("idem");
        assert_eq!(install(&root, "gaff hook").unwrap(), Outcome::Changed);
        let first = std::fs::read_to_string(root.join(SETTINGS_PATH)).unwrap();
        assert_eq!(install(&root, "gaff hook").unwrap(), Outcome::Unchanged);
        let second = std::fs::read_to_string(root.join(SETTINGS_PATH)).unwrap();
        assert_eq!(first, second);
        let v: Value = serde_json::from_str(&first).unwrap();
        for event in HOOK_EVENTS {
            assert!(
                v["hooks"][event][0]["hooks"][0]["command"] == "gaff hook",
                "{event} missing"
            );
        }
    }

    #[test]
    fn install_preserves_foreign_entries_and_uninstall_removes_only_ours() {
        let root = temp_root("foreign");
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(SETTINGS_PATH),
            r#"{"permissions":{"allow":["Read"]},"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"other-tool hook"}]}]}}"#,
        )
        .unwrap();
        install(&root, "gaff hook").unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(SETTINGS_PATH)).unwrap())
                .unwrap();
        assert_eq!(
            v["permissions"]["allow"][0], "Read",
            "gaff keeps the foreign keys"
        );
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 2);

        uninstall(&root, "gaff hook").unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(SETTINGS_PATH)).unwrap())
                .unwrap();
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "other-tool hook"
        );
        assert!(
            v["hooks"].get("PostToolBatch").is_none(),
            "gaff drops the empty arrays"
        );
    }

    #[test]
    fn uninstall_on_clean_file_changes_nothing() {
        let root = temp_root("clean");
        assert_eq!(uninstall(&root, "gaff hook").unwrap(), Outcome::Unchanged);
        assert!(!root.join(SETTINGS_PATH).exists());
    }

    #[test]
    fn invalid_settings_file_is_refused_not_clobbered() {
        let root = temp_root("invalid");
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(SETTINGS_PATH), "not json").unwrap();
        assert!(install(&root, "gaff hook").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join(SETTINGS_PATH)).unwrap(),
            "not json",
            "gaff keeps the original file"
        );
    }
}
