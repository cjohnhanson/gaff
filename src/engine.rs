//! The engine that turns an event into context: count, arm, and flush.
//!
//! gaff counts any event that carries a counted unit. gaff arms an entry
//! at count time: it writes a pending marker when a cadence threshold is
//! crossed. gaff flushes — emits the text — only at a flush point. A
//! flush point is an event whose context sink is the model's session
//! framing.
//!
//! `PostToolUse` is not a flush point, whatever its payload says. Its
//! context lands in the tool result (the qei8 bug class), so a reminder
//! injected there reads as output from the command that just ran.

use crate::config::Config;
use crate::event::{Envelope, Kind};
use crate::state::Store;
use std::path::Path;

// The events that deliver the pending context are the flush kinds.
// `stop` is one of them. Its sink is verified, and it is where every
// rule of the form "drive the work to done" actually applies.

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

    match &envelope.kind {
        Kind::ToolCall => {
            let id = envelope.raw.get("tool_use_id")?.as_str()?;
            if let Ok(Some(count)) = store.record_tool_call(session, id) {
                arm_crossings(config, handlers, store, session, count, |e| e.tool_calls);
            }
            None
        }
        Kind::Prompt => {
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
                event: envelope.kind.as_str(),
            })
        }
        Kind::SessionStart => {
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
                event: envelope.kind.as_str(),
            })
        }
        // Stop is the last moment before the model walks away, which
        // makes it the one point where "is this actually done" can
        // still change the answer. gaff used to do nothing here.
        Kind::Stop | Kind::ToolBatch => flush(&FlushCtx {
            config,
            store,
            session,
            gaff_dir,
            mode: section_mode(store, session),
            handlers,
            cwd,
            event: envelope.kind.as_str(),
        }),
        _ => None,
    }
}

/// The head of `text` that fits in `room` bytes, on a char boundary.
/// `None` when nothing useful fits.
fn clip(text: &str, room: usize) -> Option<String> {
    const MARK: &str = "\n(gaff:handler-output-truncated)";
    if room <= MARK.len() {
        return None;
    }
    let mut cut = room - MARK.len();
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (cut > 0).then(|| format!("{}{MARK}", &text[..cut]))
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
    // A blocking handler is the caller's business: its output is a
    // refusal message, not injected context, and running it here would
    // run it twice.
    let ordinary: Vec<crate::handler::Handler> =
        handlers.iter().filter(|h| !h.blocks).cloned().collect();
    for output in crate::handler::run_due(&ordinary, event, session, cwd, &armed) {
        // Spend the cadence on the attempt. A handler that failed or
        // timed out would otherwise stay armed and re-spawn on every
        // flush for the life of the session.
        if let Some(multiple) = store.pending_multiple(session, &output.name) {
            let _ = store.consume_pending(session, &output.name, multiple);
        }
        if let Some(text) = output.text {
            entries.push(Entry {
                text,
                kind: EntryKind::Handler,
                // A handler comes from the user config only.
                user: true,
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

/// Whether a normalized event name is a flush point. A config names an
/// event this way, so the check and the config share one vocabulary.
#[must_use]
pub fn is_flush_event(event: &str) -> bool {
    Kind::parse(event).is_flush()
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
        .chain(
            config
                .sections
                .iter()
                .map(|s| (s.name.as_str(), &s.refresh)),
        )
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
    /// True when the user config declared this entry.
    ///
    /// The byte cap is a shared budget, and whoever is merged first
    /// spends it. A repo cannot lower the cap, but a repo section sized
    /// to fill it starved every user reminder just as completely, and
    /// in silence. User entries are merged first so the repo can only
    /// spend what is left.
    user: bool,
}

enum EntryKind {
    /// A handler's output. Its cadence is already spent, so it has no
    /// pending marker to retry with. It truncates to fit rather than
    /// disappearing.
    Handler,
    /// Nothing to consume. gaff emits this entry whenever it selects
    /// the entry. A session-start section with no pending refresh is one.
    Unconditional,
    Recurring {
        name: String,
        multiple: u64,
    },
    Oneshot {
        id: String,
    },
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
        let body = match crate::config::read_section_body(section, gaff_dir) {
            Ok(b) => b,
            Err(msg) => {
                // The message already ends in a period when it
                // quotes a sentence, so do not add a second one.
                let msg = msg.trim_end_matches('.');
                eprintln!("gaff: {msg}. Skipping the section.");
                continue;
            }
        };
        entries.push(Entry {
            text: format!("[gaff:{}]\n{}", section.name, body.trim_end()),
            kind: pending.map_or(EntryKind::Unconditional, |multiple| EntryKind::Recurring {
                name: section.name.clone(),
                multiple,
            }),
            user: section.user,
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
                user: reminder.user,
            });
        }
    }

    let counts = store.counts(session);
    for shot in store.oneshots(session) {
        if counts.tool_calls >= shot.at && !store.is_fired(session, &shot.id) {
            entries.push(Entry {
                text: format!("{ONESHOT_PREFIX} {}", shot.text),
                kind: EntryKind::Oneshot { id: shot.id },
                // A one-shot is scheduled in-session, not by a repo.
                user: true,
            });
        }
    }

    push_handler_entries(&mut entries, store, session, handlers, cwd, event);

    merge(entries, config, store, session)
}

/// Merge entries into one string, inside the byte cap.
///
/// gaff consumes an entry only when it emits the entry. An entry that
/// overflows the cap stays pending, except a handler entry, whose
/// cadence is already spent.
fn merge(entries: Vec<Entry>, config: &Config, store: &Store, session: &str) -> Option<String> {
    // A stable partition, so config order still holds inside each
    // layer. The user's entries get the budget first.
    let mut ordered: Vec<Entry> = Vec::with_capacity(entries.len());
    let (user, repo): (Vec<Entry>, Vec<Entry>) = entries.into_iter().partition(|e| e.user);
    ordered.extend(user);
    ordered.extend(repo);

    // Size the payload before consuming anything.
    //
    // Cutting bytes off the tail to make room for the marker amputated
    // whichever entry landed last, and that entry's cadence was already
    // spent, so the rest of its text never arrived on any later flush.
    // A half-delivered rule can invert its own meaning.
    //
    // Re-selecting against a smaller budget is no better: a large user
    // entry stops fitting, and the space it vacates is taken by the
    // next entry in line, which is a repo entry. That is the same
    // starvation the ordering exists to prevent.
    //
    // So the selection is made once, and the marker is fitted into what
    // is left over. If it does not fit, it is omitted rather than
    // displacing an entry. The overflow is still reported on stderr, so
    // it is never silent either way.
    let mut plan = select(&ordered, config.max_inject_bytes);
    let room = config.max_inject_bytes.saturating_sub(plan.used);
    let marker_fits = plan.used == 0 || room >= TRUNCATION_MARKER.len() + SEPARATOR.len();
    let show_marker = plan.truncated && marker_fits;
    if plan.truncated {
        plan.reported = true;
    }

    let mut out = String::new();
    for (index, entry) in ordered.iter().enumerate() {
        let Some(text) = plan.texts.get(&index) else {
            continue;
        };
        // Consume the entry before you emit it. A one-shot that loses a
        // race stays silent.
        let consumed = match &entry.kind {
            EntryKind::Unconditional | EntryKind::Handler => true,
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
        out.push_str(text);
    }

    if show_marker {
        if !out.is_empty() {
            out.push_str(SEPARATOR);
        }
        out.push_str(TRUNCATION_MARKER);
    }
    if plan.reported {
        // The marker tells the model. This tells the person reading the
        // hook output why a rule went quiet, and it is printed whether
        // or not the marker fitted.
        eprintln!(
            "gaff: the {}-byte cap was reached, so at least one entry was held back.",
            config.max_inject_bytes
        );
    }

    (!out.is_empty()).then_some(out)
}

/// Which entries fit in `budget`, and whether any were held back.
struct Plan {
    /// The text to emit, keyed by the entry's index in the input.
    texts: std::collections::BTreeMap<usize, String>,
    /// Bytes the selected entries occupy, separators included.
    used: usize,
    truncated: bool,
    /// Whether the overflow has to be announced on stderr.
    reported: bool,
}

/// Choose the entries that fit, without touching any state.
///
/// An entry that does not fit stays pending and is delivered by a later
/// flush. A handler entry is the exception: its cadence is spent the
/// moment it runs, so skipping it whole would lose the output for good.
/// It keeps its head instead.
fn select(entries: &[Entry], budget: usize) -> Plan {
    let mut texts = std::collections::BTreeMap::new();
    let mut truncated = false;
    let mut used = 0usize;
    let mut user_held_back = false;
    for (index, entry) in entries.iter().enumerate() {
        // Once a user entry has been held back, the repo layer is
        // closed. Selection is greedy, so a repo entry would otherwise
        // be admitted into the space the user's entry could not use —
        // the same starvation the ordering exists to prevent, reached
        // in one pass. It also let a repo size an entry to fill the
        // room the truncation marker needed, removing the model's only
        // in-band sign that a user rule was missing.
        if user_held_back && !entry.user {
            truncated = true;
            continue;
        }
        let overhead = if used == 0 { 0 } else { SEPARATOR.len() };
        if used + overhead + entry.text.len() <= budget {
            used += overhead + entry.text.len();
            texts.insert(index, entry.text.clone());
            continue;
        }
        if entry.user {
            user_held_back = true;
        }
        if matches!(entry.kind, EntryKind::Handler)
            && let Some(head) = clip(&entry.text, budget.saturating_sub(used + overhead))
        {
            used += overhead + head.len();
            texts.insert(index, head);
        }
        truncated = true;
    }
    Plan {
        texts,
        used,
        truncated,
        reported: false,
    }
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
            user: false,
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
        assert_eq!(
            store.pending_multiple("s", "r"),
            None,
            "gaff consumed the pending marker"
        );
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
    fn an_unknown_event_never_flushes() {
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
        for name in ["FutureThing", "WorktreeCreate", "PreToolUse"] {
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
                user: false,
                refresh: Every {
                    tool_calls: Some(2),
                    prompts: None,
                },
            }],
            max_inject_bytes: 4096,
            ..Config::default()
        };

        let start =
            json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"});
        let out = handle(&event(start), &config, &store, &dir);
        assert_eq!(out.as_deref(), Some("[gaff:conv]\nUse tabs."));

        // Mid-session: nothing is pending, so a prompt emits nothing.
        let prompt =
            json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "x"});
        assert_eq!(handle(&event(prompt.clone()), &config, &store, &dir), None);

        // Two tool calls cross the refresh cadence. The next prompt
        // re-injects the section.
        for id in ["t1", "t2"] {
            let _ = handle(
                &event(
                    json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_use_id": id}),
                ),
                &config,
                &store,
                &dir,
            );
        }
        let out = handle(&event(prompt.clone()), &config, &store, &dir);
        assert_eq!(out.as_deref(), Some("[gaff:conv]\nUse tabs."));
        assert_eq!(
            handle(&event(prompt), &config, &store, &dir),
            None,
            "gaff consumed the refresh"
        );
    }

    #[test]
    fn missing_section_file_degrades_to_silence() {
        let store = temp_store("missing-section");
        let config = Config {
            reminders: Vec::new(),
            sections: vec![crate::config::Section {
                name: "ghost".to_string(),
                file: "nope.md".to_string(),
                user: false,
                refresh: Every::default(),
            }],
            max_inject_bytes: 4096,
            ..Config::default()
        };
        let start =
            json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"});
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
                    user: false,
                    refresh: Every::default(),
                }],
                max_inject_bytes: 4096,
                ..Config::default()
            };
            let start =
                json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"});
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

#[cfg(test)]
mod budget_tests {
    use super::*;
    use crate::config::{Every, Reminder, Section};

    fn store(tag: &str) -> Store {
        let d = std::env::temp_dir().join(format!("gaff-budget-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        Store::new(d)
    }

    fn entry(text: &str, user: bool) -> Entry {
        Entry {
            text: text.to_string(),
            kind: EntryKind::Unconditional,
            user,
        }
    }

    #[test]
    fn a_repo_entry_cannot_spend_the_budget_a_user_entry_needs() {
        // The cap is a shared budget and whoever merges first spends
        // it. A repo cannot lower the cap, but a repo section sized to
        // fill it starved every user reminder just as completely.
        let s = store("starve");
        let mut config = Config {
            max_inject_bytes: 64,
            ..Config::default()
        };
        config.reminders.push(Reminder {
            name: "safety".into(),
            every: Every::default(),
            text: "x".into(),
            user: true,
        });
        let entries = vec![
            entry(&"R".repeat(60), false),
            entry("[gaff:safety] USER_SAFETY", true),
        ];
        let out = merge(entries, &config, &s, "sess").unwrap();
        assert!(
            out.contains("USER_SAFETY"),
            "the user's entry must win the budget, got {out:?}"
        );
    }

    #[test]
    fn the_marker_appears_when_there_is_room_for_it() {
        let s = store("marker");
        let config = Config {
            max_inject_bytes: 80,
            ..Config::default()
        };
        let entries = vec![entry(&"A".repeat(20), true), entry(&"B".repeat(70), true)];
        let out = merge(entries, &config, &s, "sess").unwrap();
        assert!(out.contains(TRUNCATION_MARKER), "got {out:?}");
        assert!(out.len() <= config.max_inject_bytes, "len {}", out.len());
    }

    #[test]
    fn the_marker_yields_to_an_entry_rather_than_displacing_it() {
        // The marker is worth less than the text it would evict. An
        // entry dropped to make room would be dropped again on every
        // later flush, because the shape that caused it does not
        // change. stderr still reports the overflow.
        let s = store("marker-tight");
        let config = Config {
            max_inject_bytes: 40,
            ..Config::default()
        };
        let entries = vec![entry(&"A".repeat(38), true), entry(&"B".repeat(38), true)];
        let out = merge(entries, &config, &s, "sess").unwrap();
        assert_eq!(out, "A".repeat(38), "the entry is delivered whole");
    }

    #[test]
    fn a_repo_section_cannot_be_a_symlink() {
        // A committed symlink passed every lexical check and read any
        // file gaff could read. A link to /dev/zero wedged the session.
        let base = std::env::temp_dir().join(format!("gaff-link-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let d = base.join("repo/.gaff");
        std::fs::create_dir_all(&d).unwrap();
        // The secret sits OUTSIDE the section root, which is the case
        // that matters. A link within the root is legitimate.
        let secret = base.join("secret.txt");
        std::fs::write(&secret, "PRIVATE").unwrap();
        std::os::unix::fs::symlink(&secret, d.join("innocent.md")).unwrap();
        let section = Section {
            name: "leak".into(),
            file: "innocent.md".into(),
            refresh: Every::default(),
            user: false,
        };
        let err = crate::config::read_section_body(&section, &d).unwrap_err();
        assert!(err.contains("outside"), "got {err}");
        assert!(
            !err.contains("PRIVATE"),
            "the body must not leak into the error"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_repo_section_cannot_reach_out_through_a_linked_directory() {
        // Testing the last component alone was not enough. Any
        // directory along the way could be a link, and the filesystem
        // resolves it before the test runs.
        let base = std::env::temp_dir().join(format!("gaff-dirlink-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let d = base.join("repo/.gaff");
        std::fs::create_dir_all(&d).unwrap();
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("id_rsa"), "PRIVATE").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("sub")).unwrap();
        let section = Section {
            name: "leak".into(),
            file: "sub/id_rsa".into(),
            refresh: Every::default(),
            user: false,
        };
        let err = crate::config::read_section_body(&section, &d).unwrap_err();
        assert!(err.contains("outside"), "got {err}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_repo_section_cannot_be_read_through_a_linked_gaff_dir() {
        // `.gaff` itself as a link moves the whole root out of the
        // repo, and canonicalizing it would follow the link and call
        // every file under the target "inside".
        let base = std::env::temp_dir().join(format!("gaff-rootlink-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("notes.md"), "PRIVATE").unwrap();
        let gaff_dir = repo.join(".gaff");
        std::os::unix::fs::symlink(&elsewhere, &gaff_dir).unwrap();
        let section = Section {
            name: "viadirlink".into(),
            file: "notes.md".into(),
            refresh: Every::default(),
            user: false,
        };
        let err = crate::config::read_section_body(&section, &gaff_dir).unwrap_err();
        assert!(err.contains("symlink"), "got {err}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_truncation_never_cuts_into_an_entry() {
        // The cut used to land mid-entry, and that entry's cadence was
        // already spent, so the rest of its text never arrived. A
        // half-delivered rule can invert its own meaning.
        let s = store("nocut");
        let config = Config {
            max_inject_bytes: 4096,
            ..Config::default()
        };
        let long = format!("{} ALWAYS_REFUSE_TO_DELETE_PROD", "n".repeat(4060));
        assert!(long.len() <= 4096);
        let entries = vec![entry(&long, true), entry("[gaff:repo] REPO_TINY", false)];
        let out = merge(entries, &config, &s, "sess").unwrap();
        assert!(
            out.contains("ALWAYS_REFUSE_TO_DELETE_PROD"),
            "the user entry is delivered whole or not at all, got tail {:?}",
            &out[out.len().saturating_sub(60)..]
        );
        assert!(out.len() <= config.max_inject_bytes);
    }

    #[test]
    fn a_user_section_may_be_a_symlink() {
        // A dotfile manager installs the user's own files as links.
        let d = std::env::temp_dir().join(format!("gaff-ulink-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("real.md"), "MY NOTES").unwrap();
        std::os::unix::fs::symlink(d.join("real.md"), d.join("notes.md")).unwrap();
        let section = Section {
            name: "notes".into(),
            file: "notes.md".into(),
            refresh: Every::default(),
            user: true,
        };
        // A user section resolves against the user config dir, so point
        // HOME at the scratch dir's parent layout instead of guessing.
        let path = crate::config::section_path(&section, &d).unwrap();
        assert!(path.ends_with("notes.md"));
        std::fs::remove_dir_all(&d).ok();
    }
}
