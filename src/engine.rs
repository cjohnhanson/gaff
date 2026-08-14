//! The engine that turns an event into context: count, arm, and flush.
//!
//! gaff counts any event that carries a counted unit. gaff arms an entry
//! at count time: it writes a pending marker when a cadence threshold is
//! crossed. gaff flushes — emits the text — only at a flush point. A
//! flush point is an event whose context sink is the model's session
//! framing.
//!
//! `PostToolUse` is not a flush point, whatever its payload says. Its
//! context lands in the tool result (the qei8 bug class).

use crate::config::Config;
use crate::event::Envelope;
use crate::state::Store;
use std::path::Path;

/// The events that deliver the pending context. `Stop` has a safe sink,
/// but gaff excludes it. A decorated stop decision does not re-prime the
/// context.
const FLUSH_EVENTS: [&str; 3] = ["SessionStart", "UserPromptSubmit", "PostToolBatch"];

/// The fixed attribution prefix for an agent-scheduled one-shot.
const ONESHOT_PREFIX: &str = "[gaff:remind]";
const TRUNCATION_MARKER: &str = "[gaff:truncated]";
const SEPARATOR: &str = "\n\n";

/// Handle one event end to end. Returns the context to inject, if any.
///
/// `gaff_dir` is the repo's `.gaff/` directory. A section file path is
/// relative to it. Every error path returns `None`. The engine never
/// turns an IO problem into a blocked session.
#[must_use]
pub fn handle(
    envelope: &Envelope,
    config: &Config,
    store: &Store,
    gaff_dir: &Path,
) -> Option<String> {
    handle_with(envelope, config, store, gaff_dir, &[], None)
}

/// Handle one event, with handlers.
///
/// `cwd` is the working directory a handler's child runs in. It is
/// passed explicitly rather than derived from `gaff_dir`, because the
/// two can diverge and running a command in the wrong repo is a silent
/// boundary crossing.
#[must_use]
pub fn handle_with(
    envelope: &Envelope,
    config: &Config,
    store: &Store,
    gaff_dir: &Path,
    handlers: &[crate::handler::Handler],
    cwd: Option<&Path>,
) -> Option<String> {
    let session = envelope.session_id.as_deref()?;

    match envelope.event.as_str() {
        "PostToolUse" | "PostToolUseFailure" => {
            let id = envelope.raw.get("tool_use_id")?.as_str()?;
            if let Ok(Some(count)) = store.record_tool_call(session, id) {
                arm_crossings(config, handlers, store, session, count, |e| e.tool_calls);
            }
            None
        }
        "UserPromptSubmit" => {
            if let Ok(count) = store.record_prompt(session) {
                arm_crossings(config, handlers, store, session, count, |e| e.prompts);
            }
            flush(&FlushCtx {
                config,
                store,
                session,
                gaff_dir,
                mode: section_mode(store, session),
                handlers,
                cwd,
                event: &envelope.event,
            })
        }
        "SessionStart" => {
            if compaction_source(envelope) {
                store.clear_fired(session);
            }
            flush(&FlushCtx {
                config,
                store,
                session,
                gaff_dir,
                mode: SectionMode::All,
                handlers,
                cwd,
                event: &envelope.event,
            })
        }
        "PostToolBatch" => flush(&FlushCtx {
                config,
                store,
                session,
                gaff_dir,
                mode: section_mode(store, session),
                handlers,
                cwd,
                event: &envelope.event,
            }),
        _ => None,
    }
}

/// Run the armed handlers and push their output.
///
/// Handlers run last, so derived context yields to scheduled context. A
/// handler arms like a reminder, so it spawns a process only when its
/// cadence crosses.
fn push_handler_entries(
    entries: &mut Vec<Entry>,
    store: &Store,
    session: &str,
    handlers: &[crate::handler::Handler],
    cwd: Option<&Path>,
    event: &str,
) {
    let Some(cwd) = cwd else { return };
    if handlers.is_empty() {
        return;
    }
    let armed = |name: &str| store.pending_multiple(session, name).is_some();
    for output in crate::handler::run_due(handlers, event, session, cwd, &armed) {
        if let Some(multiple) = store.pending_multiple(session, &output.name)
            && store.consume_pending(session, &output.name, multiple).is_ok()
        {
            entries.push(Entry {
                text: output.text,
                kind: EntryKind::Unconditional,
            });
        }
    }
}

/// A profile switch re-primes the context. The switch changes which
/// sections apply, so the next flush delivers them all rather than wait
/// for each refresh cadence to come around.
fn section_mode(store: &Store, session: &str) -> SectionMode {
    if store.take_reprime(session) {
        SectionMode::All
    } else {
        SectionMode::PendingOnly
    }
}

#[must_use]
pub fn is_flush_event(event: &str) -> bool {
    FLUSH_EVENTS.contains(&event)
}

/// Read the origin field of a `SessionStart` event. The live reference
/// documents `startup_mode`. The predecessor parsed `source`. gaff
/// accepts either field.
fn compaction_source(envelope: &Envelope) -> bool {
    ["startup_mode", "source"]
        .iter()
        .any(|k| envelope.raw.get(k).and_then(|v| v.as_str()) == Some("compact"))
}

/// Write a pending marker for every reminder and section whose cadence
/// divides the new count.
fn arm_crossings(
    config: &Config,
    handlers: &[crate::handler::Handler],
    store: &Store,
    session: &str,
    count: u64,
    unit: impl Fn(&crate::config::Every) -> Option<u64>,
) {
    let cadences = config
        .reminders
        .iter()
        .map(|r| (r.name.as_str(), &r.every))
        .chain(config.sections.iter().map(|s| (s.name.as_str(), &s.refresh)))
        .chain(handlers.iter().map(|h| (h.name.as_str(), &h.every)));
    for (name, every) in cadences {
        if let Some(n) = unit(every)
            && n > 0
            && count.is_multiple_of(n)
        {
            store.write_pending(session, name, count / n).ok();
        }
    }
}

/// An entry that gaff can deliver at a flush point.
struct Entry {
    text: String,
    kind: EntryKind,
}

enum EntryKind {
    /// Nothing to consume. gaff emits this entry whenever it selects
    /// the entry. A session-start section with no pending refresh is one.
    Unconditional,
    Recurring { name: String, multiple: u64 },
    Oneshot { id: String },
}

/// Which sections a flush delivers. `All` primes the context at session
/// start. `PendingOnly` delivers only the sections whose refresh cadence
/// crossed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionMode {
    All,
    PendingOnly,
}

/// Deliver the pending context. The sections come first, because they
/// are the prime text. The recurring reminders follow in config order.
/// The one-shots come last, sorted by id.
///
/// gaff consumes an entry only when it emits the entry. An entry that
/// overflows the byte cap stays pending, and gaff appends the truncation
/// marker instead.
/// Everything one flush needs. The reviewer's note applies: derive no
/// path here. `cwd` and `gaff_dir` can diverge, and running a handler
/// in the wrong repo is a silent boundary crossing.
struct FlushCtx<'a> {
    config: &'a Config,
    store: &'a Store,
    session: &'a str,
    gaff_dir: &'a Path,
    mode: SectionMode,
    handlers: &'a [crate::handler::Handler],
    cwd: Option<&'a Path>,
    event: &'a str,
}

fn flush(ctx: &FlushCtx<'_>) -> Option<String> {
    let FlushCtx {
        config,
        store,
        session,
        gaff_dir,
        mode,
        handlers,
        cwd,
        event,
    } = *ctx;
    let mut entries: Vec<Entry> = Vec::new();

    for section in &config.sections {
        let pending = store.pending_multiple(session, &section.name);
        if mode == SectionMode::PendingOnly && pending.is_none() {
            continue;
        }
        let Ok(path) = crate::config::confine_section_path(gaff_dir, &section.file) else {
            eprintln!(
                "gaff: section `{}`: the path {} leaves .gaff/. Skipping the section.",
                section.name, section.file
            );
            continue;
        };
        let Ok(body) = std::fs::read_to_string(&path) else {
            eprintln!(
                "gaff: section `{}`: cannot read {}. Skipping the section.",
                section.name, section.file
            );
            continue;
        };
        entries.push(Entry {
            text: format!("[gaff:{}]\n{}", section.name, body.trim_end()),
            kind: pending.map_or(EntryKind::Unconditional, |multiple| EntryKind::Recurring {
                name: section.name.clone(),
                multiple,
            }),
        });
    }

    for reminder in &config.reminders {
        if let Some(multiple) = store.pending_multiple(session, &reminder.name) {
            entries.push(Entry {
                text: format!("[gaff:{}] {}", reminder.name, reminder.text),
                kind: EntryKind::Recurring {
                    name: reminder.name.clone(),
                    multiple,
                },
            });
        }
    }

    let counts = store.counts(session);
    for shot in store.oneshots(session) {
        if counts.tool_calls >= shot.at && !store.is_fired(session, &shot.id) {
            entries.push(Entry {
                text: format!("{ONESHOT_PREFIX} {}", shot.text),
                kind: EntryKind::Oneshot { id: shot.id },
            });
        }
    }

    push_handler_entries(&mut entries, store, session, handlers, cwd, event);

    let (mut out, mut truncated) = (String::new(), false);
    for entry in entries {
        let candidate_len = if out.is_empty() {
            entry.text.len()
        } else {
            out.len() + SEPARATOR.len() + entry.text.len()
        };
        if candidate_len > config.max_inject_bytes {
            truncated = true;
            continue;
        }
        // Consume the entry before you emit it. A one-shot that loses a
        // race stays silent.
        let consumed = match &entry.kind {
            EntryKind::Unconditional => true,
            EntryKind::Recurring { name, multiple } => {
                store.consume_pending(session, name, *multiple).is_ok()
            }
            EntryKind::Oneshot { id } => store.claim_fired(session, id),
        };
        if !consumed {
            continue;
        }
        if !out.is_empty() {
            out.push_str(SEPARATOR);
        }
        out.push_str(&entry.text);
    }

    if truncated {
        let marker_len = if out.is_empty() {
            TRUNCATION_MARKER.len()
        } else {
            out.len() + SEPARATOR.len() + TRUNCATION_MARKER.len()
        };
        if marker_len <= config.max_inject_bytes {
            if !out.is_empty() {
                out.push_str(SEPARATOR);
            }
            out.push_str(TRUNCATION_MARKER);
        }
    }

    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Every, Reminder};
    use crate::state::Store;
    use serde_json::json;


    fn gd() -> &'static Path {
        Path::new("/nonexistent-gaff-dir")
    }
    fn temp_store(tag: &str) -> Store {
        let root =
            std::env::temp_dir().join(format!("gaff-engine-test-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        Store::new(root)
    }

    fn cfg(reminders: Vec<Reminder>, cap: usize) -> Config {
        let mut c = Config {
            reminders,
            sections: Vec::new(),
            max_inject_bytes: cap,
            ..Config::default()
        };
        if cap == 0 {
            c.max_inject_bytes = 4096;
        }
        c
    }

    fn reminder(name: &str, every_tool_calls: u64, text: &str) -> Reminder {
        Reminder {
            name: name.to_string(),
            every: Every {
                tool_calls: Some(every_tool_calls),
                prompts: None,
            },
            text: text.to_string(),
        }
    }

    fn event(v: serde_json::Value) -> Envelope {
        Envelope::from_claude_code(v)
    }

    #[test]
    fn crossing_arms_but_post_tool_use_stays_silent() {
        let store = temp_store("arm");
        let config = cfg(vec![reminder("r", 2, "hello")], 0);
        for id in ["t1", "t2"] {
            let out = handle(
                &event(
                    json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_use_id": id}),
                ),
                &config,
                &store,
                gd(),
            );
            assert_eq!(out, None, "PostToolUse must never emit context");
        }
        assert_eq!(store.pending_multiple("s", "r"), Some(1));
    }

    #[test]
    fn prompt_flushes_pending_with_attribution() {
        let store = temp_store("flush");
        let config = cfg(vec![reminder("r", 1, "hello")], 0);
        let _ = handle(
            &event(
                json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_use_id": "t1"}),
            ),
            &config,
            &store,
            gd(),
        );
        let out = handle(
            &event(
                json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "x"}),
            ),
            &config,
            &store,
            gd(),
        );
        assert_eq!(out.as_deref(), Some("[gaff:r] hello"));
        assert_eq!(store.pending_multiple("s", "r"), None, "gaff consumed the pending marker");
    }

    #[test]
    fn cap_skips_oversized_entry_keeps_it_pending_and_marks() {
        let store = temp_store("cap");
        let config = cfg(
            vec![
                reminder("a", 1, "short"),
                reminder("b", 1, &"x".repeat(100)),
            ],
            40,
        );
        let _ = handle(
            &event(
                json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_use_id": "t1"}),
            ),
            &config,
            &store,
            gd(),
        );
        let out = handle(
            &event(
                json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "x"}),
            ),
            &config,
            &store,
            gd(),
        )
        .unwrap();
        assert!(out.contains("[gaff:a] short"));
        assert!(!out.contains("xxx"), "an oversized entry must not leak");
        assert!(out.contains(TRUNCATION_MARKER));
        assert!(out.len() <= 40);
        assert_eq!(store.pending_multiple("s", "b"), Some(1), "b stays pending");
    }

    #[test]
    fn oneshot_fires_once_and_compact_rearms() {
        let store = temp_store("oneshot");
        let config = cfg(vec![], 0);
        store.write_oneshot("s", "ci", 0, 0, "check CI").unwrap();
        let prompt =
            json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "x"});
        let out = handle(&event(prompt.clone()), &config, &store, gd());
        assert_eq!(out.as_deref(), Some("[gaff:remind] check CI"));
        assert_eq!(
            handle(&event(prompt), &config, &store, gd()),
            None,
            "no refire"
        );

        let compact =
            json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "compact"});
        let out = handle(&event(compact), &config, &store, gd());
        assert_eq!(
            out.as_deref(),
            Some("[gaff:remind] check CI"),
            "a compaction re-primes the context"
        );
        let plain = json!({"hook_event_name": "SessionStart", "session_id": "s", "startup_mode": "startup"});
        assert_eq!(handle(&event(plain), &config, &store, gd()), None);
    }

    #[test]
    fn stop_and_unknown_events_never_flush() {
        let store = temp_store("noflush");
        let config = cfg(vec![reminder("r", 1, "hello")], 0);
        let _ = handle(
            &event(
                json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_use_id": "t1"}),
            ),
            &config,
            &store,
            gd(),
        );
        for name in ["Stop", "FutureThing", "WorktreeCreate", "PreToolUse"] {
            let out = handle(
                &event(json!({"hook_event_name": name, "session_id": "s", "tool_use_id": "t9"})),
                &config,
                &store,
                gd(),
            );
            assert_eq!(out, None, "{name} must not flush");
        }
        assert_eq!(store.pending_multiple("s", "r"), Some(1));
    }

    #[test]
    fn sections_prime_at_session_start_and_refresh_on_cadence() {
        let store = temp_store("sections");
        let dir = std::env::temp_dir().join(format!("gaff-sections-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("conv.md"), "Use tabs.\n").unwrap();
        let config = Config {
            reminders: Vec::new(),
            sections: vec![crate::config::Section {
                name: "conv".to_string(),
                file: "conv.md".to_string(),
                refresh: Every {
                    tool_calls: Some(2),
                    prompts: None,
                },
            }],
            max_inject_bytes: 4096,
            ..Config::default()
        };

        let start = json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"});
        let out = handle(&event(start), &config, &store, &dir);
        assert_eq!(out.as_deref(), Some("[gaff:conv]\nUse tabs."));

        // Mid-session: nothing is pending, so a prompt emits nothing.
        let prompt = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "x"});
        assert_eq!(handle(&event(prompt.clone()), &config, &store, &dir), None);

        // Two tool calls cross the refresh cadence. The next prompt
        // re-injects the section.
        for id in ["t1", "t2"] {
            let _ = handle(
                &event(json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_use_id": id})),
                &config,
                &store,
                &dir,
            );
        }
        let out = handle(&event(prompt.clone()), &config, &store, &dir);
        assert_eq!(out.as_deref(), Some("[gaff:conv]\nUse tabs."));
        assert_eq!(handle(&event(prompt), &config, &store, &dir), None, "gaff consumed the refresh");
    }

    #[test]
    fn missing_section_file_degrades_to_silence() {
        let store = temp_store("missing-section");
        let config = Config {
            reminders: Vec::new(),
            sections: vec![crate::config::Section {
                name: "ghost".to_string(),
                file: "nope.md".to_string(),
                refresh: Every::default(),
            }],
            max_inject_bytes: 4096,
            ..Config::default()
        };
        let start = json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"});
        assert_eq!(handle(&event(start), &config, &store, gd()), None);
    }

    #[test]
    fn section_path_escaping_gaff_dir_reads_nothing() {
        // A committed config must not read a file outside .gaff/ into the
        // model's context. A canary sits beside the temp .gaff/ dir; the
        // section tries to reach it with `..`.
        let dir = std::env::temp_dir().join(format!("gaff-escape-{}", std::process::id()));
        let gaff = dir.join(".gaff");
        std::fs::create_dir_all(&gaff).unwrap();
        std::fs::write(dir.join("canary.txt"), "SECRET").unwrap();
        let store = temp_store("escape");
        for bad in ["../canary.txt", "../../etc/hostname", "/etc/hostname"] {
            let config = Config {
                reminders: Vec::new(),
                sections: vec![crate::config::Section {
                    name: "leak".to_string(),
                    file: bad.to_string(),
                    refresh: Every::default(),
                }],
                max_inject_bytes: 4096,
            ..Config::default()
            };
            let start = json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"});
            let out = handle(&event(start), &config, &store, &gaff);
            assert_eq!(out, None, "escaping path `{bad}` must inject nothing");
        }
    }

    #[test]
    fn sessionless_events_touch_nothing() {
        let store = temp_store("nosession");
        let config = cfg(vec![reminder("r", 1, "hello")], 0);
        let out = handle(
            &event(json!({"hook_event_name": "PostToolUse", "tool_use_id": "t1"})),
            &config,
            &store,
            gd(),
        );
        assert_eq!(out, None);
        assert_eq!(store.counts("t1").tool_calls, 0);
    }
}

/// A regression test. `Config::default()` once set `max_inject_bytes` to
/// zero, so the no-config path suppressed every flush. The missouri
/// suite found the bug.
#[cfg(test)]
mod regression {
    use super::*;
    use crate::state::Store;
    use serde_json::json;
    use std::path::Path;

    fn gd() -> &'static Path {
        Path::new("/nonexistent-gaff-dir")
    }


    #[test]
    fn oneshot_with_nonzero_at_fires_after_crossing() {
        let root = std::env::temp_dir().join(format!("gaff-repro-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let store = Store::new(root);
        let config = crate::config::Config::default();
        store.write_oneshot("s", "ci", 2, 2, "check CI").unwrap();
        for id in ["t1", "t2"] {
            let out = handle(
                &Envelope::from_claude_code(
                    json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_use_id": id}),
                ),
                &config,
                &store,
                gd(),
            );
            assert_eq!(out, None);
        }
        let out = handle(
            &Envelope::from_claude_code(
                json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "x"}),
            ),
            &config,
            &store,
            gd(),
        );
        assert_eq!(out.as_deref(), Some("[gaff:remind] check CI"));
    }
}
