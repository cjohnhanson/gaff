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
                // An unknown shape: leave the user's config alone, and
                // say which event went unregistered. Reporting "hooks
                // registered" while the one blocking event was skipped
                // is a false status.
                eprintln!(
                    "gaff: the `{event}` key is not an array, so gaff did not register there. Fix it by hand, or remove the key."
                );
                continue;
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
                // Filter the inner hooks array, then drop an entry only
                // once it is empty. Dropping the whole matcher group
                // because one hook inside it was gaff's destroyed
                // another tool's hook that shared the group.
                let mut emptied = Vec::new();
                for (index, entry) in list.iter_mut().enumerate() {
                    let Some(inner) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                        continue;
                    };
                    let before = inner.len();
                    inner.retain(|h| !runs_command(&h["command"], command));
                    if inner.len() != before {
                        changed = true;
                        if inner.is_empty() {
                            emptied.push(index);
                        }
                    }
                }
                // Drop only what this call emptied. Sweeping every
                // empty entry took one that arrived empty, which is
                // the user's however inert it looks.
                let before = list.len();
                let mut index = 0;
                list.retain(|_| {
                    let keep = !emptied.contains(&index);
                    index += 1;
                    keep
                });
                changed |= list.len() != before;
                !list.is_empty()
            } else {
                true
            }
        });
        if hooks.is_empty() {
            // `remove` is `swap_remove` under `preserve_order`, so it
            // moves the last key into this slot and scrambles the
            // file gaff just went to trouble to keep in order.
            settings.shift_remove("hooks");
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

/// True when a matcher-group entry holds a hook that runs this command.
fn has_command(entry: &Value, command: &str) -> bool {
    entry["hooks"]
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(|h| runs_command(&h["command"], command)))
}

/// Whether a registered hook command is this command, by any path.
///
/// A registration may name the binary bare (`gaff hook`), by an
/// absolute path (`/nix/store/…/bin/gaff hook`), or by a profile link.
/// Exact string equality treated each as a different hook, so uninstall
/// left the absolute-path registration in place and reported "already
/// up to date" while the hook ran twice. Compare the argv tail: the
/// program's basename plus its arguments.
fn runs_command(registered: &Value, command: &str) -> bool {
    let Some(reg) = registered.as_str() else {
        return false;
    };
    let tail = |s: &str| -> Vec<String> {
        let mut words: Vec<String> = s.split_whitespace().map(String::from).collect();
        if let Some(first) = words.first_mut() {
            *first = first.rsplit('/').next().unwrap_or(first).to_string();
        }
        words
    };
    tail(reg) == tail(command)
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
    // A settings file is a regular file. Reading a FIFO here blocked
    // forever, waiting for a writer that never came.
    if let Ok(meta) = std::fs::symlink_metadata(&path)
        && !meta.is_file()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: not a regular file. Refusing to rewrite it.", path.display()),
        ));
    }
    // Absent and unreadable are different things, and treating them
    // alike destroyed settings files. One non-UTF8 byte, or a mode gaff
    // could not read, made the read fail; gaff then proceeded as though
    // the repo had no settings, wrote its own map, and renamed over the
    // user's `permissions` and `model` keys at exit 0.
    let mut settings: Map<String, Value> = match std::fs::read_to_string(&path) {
        // An empty file is a common benign state, not corruption.
        Ok(bytes) if bytes.trim().is_empty() => Map::new(),
        Ok(bytes) => serde_json::from_str(&bytes).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{}: not valid JSON ({e}). Refusing to rewrite the file.",
                    path.display()
                ),
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{}: cannot read it ({e}). Refusing to rewrite the file, because that would replace what is there.",
                    path.display()
                ),
            ));
        }
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
    // A hard link has no target to resolve, so a rename replaces this
    // name and leaves the other link pointing at the old content. Write
    // in place instead, which keeps the link at the cost of the atomic
    // swap. Every other file gets the swap.
    if hard_linked(&path) {
        std::fs::write(&path, rendered).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "{}: cannot write it ({e}). Refusing to rewrite the file.",
                    path.display()
                ),
            )
        })?;
        return Ok(Outcome::Changed);
    }
    // A per-process name, so two gaff runs cannot collide on it, and
    // it is removed when the rename fails rather than left behind.
    let tmp = path.with_extension(format!("json.gaff-tmp.{}", std::process::id()));
    std::fs::write(&tmp, rendered)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        std::fs::remove_file(&tmp).ok();
        return Err(e);
    }
    Ok(Outcome::Changed)
}

/// Whether another name refers to this same file.
#[cfg(unix)]
fn hard_linked(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.nlink() > 1)
}

#[cfg(not(unix))]
fn hard_linked(_path: &Path) -> bool {
    false
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

#[cfg(test)]
mod preservation_tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("gaff-init-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        d
    }

    fn settings(root: &Path) -> String {
        std::fs::read_to_string(root.join(".claude/settings.local.json")).unwrap()
    }

    #[test]
    fn an_unreadable_settings_file_is_never_replaced() {
        // Treating "cannot read" like "does not exist" made gaff write
        // its own map over a file it had never seen. One non-UTF8 byte
        // was enough, and `permissions` — a security control — went
        // with it, at exit 0.
        let d = scratch("unreadable");
        let path = d.join(".claude/settings.local.json");
        let original: &[u8] = b"{\"permissions\":{\"deny\":[\"x\"]},\"n\":\"caf\xe9\"}";
        std::fs::write(&path, original).unwrap();
        let err = install(&d, "gaff hook").expect_err("an unreadable file is refused");
        assert!(err.to_string().contains("cannot read"), "{err}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "the file is byte-identical"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_non_object_hooks_key_is_never_replaced() {
        let d = scratch("hookskey");
        let path = d.join(".claude/settings.local.json");
        for value in ["[1,2,3]", "\"legacy\"", "42", "null"] {
            let original = format!("{{\"model\":\"opus\",\"hooks\":{value}}}");
            std::fs::write(&path, &original).unwrap();
            let err = install(&d, "gaff hook").expect_err("a non-object hooks key is refused");
            assert!(err.to_string().contains("not an object"), "{err}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn uninstall_keeps_a_foreign_hook_that_shares_a_matcher_group() {
        // Dropping the whole matcher group because one hook inside it
        // was gaff's destroyed another tool's hook.
        let d = scratch("shared");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"gaff hook"},{"type":"command","command":"other-tool guard"}]}]}}"#,
        )
        .unwrap();
        uninstall(&d, "gaff hook").unwrap();
        let after = settings(&d);
        assert!(after.contains("other-tool guard"), "{after}");
        assert!(!after.contains("gaff hook"), "{after}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_empty_settings_file_is_a_benign_starting_point() {
        let d = scratch("empty");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(&path, "").unwrap();
        install(&d, "gaff hook").expect("an empty file is not corruption");
        assert!(settings(&d).contains("PreToolUse"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_users_key_order_survives_a_rewrite() {
        let d = scratch("order");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(&path, r#"{"zeta":1,"alpha":2,"model":"opus"}"#).unwrap();
        install(&d, "gaff hook").unwrap();
        let after = settings(&d);
        let zeta = after.find("zeta").expect("zeta kept");
        let alpha = after.find("alpha").expect("alpha kept");
        assert!(zeta < alpha, "alphabetizing a user's file is noise: {after}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn install_and_uninstall_return_the_file_to_its_bytes() {
        let d = scratch("roundtrip");
        let path = d.join(".claude/settings.local.json");
        let original = "{\n  \"zeta\": 1,\n  \"alpha\": 2\n}\n";
        std::fs::write(&path, original).unwrap();
        install(&d, "gaff hook").unwrap();
        uninstall(&d, "gaff hook").unwrap();
        assert_eq!(settings(&d), original);
        std::fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("gaff-link-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        d
    }

    #[test]
    fn uninstall_keeps_the_key_order_of_the_rest() {
        // Under `preserve_order`, `remove` is `swap_remove`: it moves
        // the last key into the removed slot. That undid the ordering
        // work on the one path that motivated it.
        let d = scratch("order");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(&path, r#"{"a":1,"b":2,"c":3,"d":4,"e":5,"f":6}"#).unwrap();
        install(&d, "gaff hook").unwrap();
        uninstall(&d, "gaff hook").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        let order: Vec<usize> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|k| after.find(&format!("\"{k}\"")).expect("key kept"))
            .collect();
        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "the keys moved: {after}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn uninstall_keeps_an_entry_that_was_already_empty() {
        // Sweeping every empty entry took one that arrived that way,
        // which is the user's however inert it looks.
        let d = scratch("preempty");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[]},{"hooks":[{"type":"command","command":"gaff hook"}]}]}}"#,
        )
        .unwrap();
        uninstall(&d, "gaff hook").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("matcher"), "the user's entry survives: {after}");
        assert!(!after.contains("gaff hook"), "{after}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_hard_linked_settings_file_keeps_its_other_name() {
        // A rename replaces this name and leaves the other link on the
        // old content. A hard link has no target for canonicalize to
        // resolve, so the write has to happen in place.
        let d = scratch("hardlink");
        let other = d.join("other.json");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(&other, r#"{"model":"opus"}"#).unwrap();
        std::fs::hard_link(&other, &path).unwrap();
        install(&d, "gaff hook").unwrap();
        let sibling = std::fs::read_to_string(&other).unwrap();
        assert!(
            sibling.contains("PreToolUse"),
            "the other name must see the same file: {sibling}"
        );
        assert!(sibling.contains("opus"), "and keep its own keys: {sibling}");
        std::fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod path_match_tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("gaff-pathmatch-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        d
    }

    #[test]
    fn uninstall_removes_a_registration_by_absolute_path() {
        // The live file held `/nix/store/…/bin/gaff hook` beside another
        // tool's hook in one group. Exact matching against `gaff hook`
        // found nothing, printed "already up to date", and left the hook
        // running twice.
        let d = scratch("abs");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"SessionStart":[{"hooks":[{"command":"/Users/x/.nix-profile/bin/gaff hook","type":"command"},{"command":"/x/demerit snapshot","type":"command"}]}]}}"#,
        )
        .unwrap();
        uninstall(&d, "gaff hook").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("gaff hook"), "{after}");
        assert!(after.contains("demerit snapshot"), "the other tool's hook survives: {after}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn install_sees_an_absolute_path_registration_as_already_present() {
        let d = scratch("dup");
        let path = d.join(".claude/settings.local.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"/opt/bin/gaff hook","type":"command"}]}]}}"#,
        )
        .unwrap();
        install(&d, "gaff hook").unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let pre = after["hooks"]["PreToolUse"].as_array().unwrap();
        let ours = pre
            .iter()
            .flat_map(|e| e["hooks"].as_array().cloned().unwrap_or_default())
            .filter(|h| runs_command(&h["command"], "gaff hook"))
            .count();
        assert_eq!(ours, 1, "the absolute-path entry counts as present: {pre:?}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_different_program_with_the_same_argument_is_not_ours() {
        assert!(!runs_command(&serde_json::json!("/x/gaffer hook"), "gaff hook"));
        assert!(!runs_command(&serde_json::json!("gaff hooks"), "gaff hook"));
        assert!(runs_command(&serde_json::json!("  /a/b/gaff   hook "), "gaff hook"));
    }
}
