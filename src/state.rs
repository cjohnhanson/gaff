//! The session state: an append-only ledger, the pending and cursor
//! files, the one-shots, and the fired markers.
//!
//! The layout is flat. A test fixture cannot represent an empty
//! directory, and every file below takes part in a byte diff.
//!
//! ```text
//! <root>/degraded                    marker: config failed to parse
//! <root>/<session>/ledger.jsonl      one line per counted event
//! <root>/<session>/pending-<name>    recurring reminder armed (multiple)
//! <root>/<session>/cursor-<name>     last flushed multiple
//! <root>/<session>/oneshot-<id>.json scheduled one-shot
//! <root>/<session>/fired-<id>        one-shot consumed (O_EXCL claim)
//! <root>/<session>/profile           the active profile name
//! <root>/<session>/reprime           marker: re-deliver every section
//! <root>/<session>/injections.jsonl  one line per injected flush
//! ```
//!
//! `GAFF_STATE_DIR` sets the root. A relative path joins the process
//! working directory. Without that variable, the root falls back to a
//! user-scoped path keyed by the working directory. The root is never
//! inside the repo, where `git clean -xdf` would erase it mid-session.
//!
//! Absent state always reads as zero. It never reads as an error.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde_json::{json, Value};

pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub tool_calls: u64,
    pub prompts: u64,
}

#[derive(Debug, Clone)]
pub struct Oneshot {
    pub id: String,
    pub at: u64,
    pub text: String,
}

/// Resolve the state root from explicit inputs. This function is pure,
/// so a test can call it without a change to the process environment.
#[must_use]
pub fn resolve_root(
    gaff_state_dir: Option<&str>,
    xdg_state_home: Option<&str>,
    home: Option<&str>,
    cwd: &Path,
) -> Option<PathBuf> {
    if let Some(dir) = gaff_state_dir {
        let p = PathBuf::from(dir);
        return Some(if p.is_absolute() { p } else { cwd.join(p) });
    }
    let base = match (xdg_state_home, home) {
        (Some(x), _) => PathBuf::from(x),
        (None, Some(h)) => Path::new(h).join(".local/state"),
        (None, None) => return None,
    };
    Some(base.join("gaff").join(cwd_key(cwd)))
}

/// Make a stable key for a working directory. This uses FNV-1a, not
/// `DefaultHasher`, which changes between Rust releases.
fn cwd_key(cwd: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in cwd.to_string_lossy().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Whether a session id is safe to use as a directory name.
///
/// The id arrives from the host payload, so it is untrusted input. An
/// id such as `../../x` would otherwise escape the state root and write
/// a ledger into an arbitrary directory.
#[must_use]
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// A stop hold the session set for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    pub id: String,
    /// What the model reads when the stop is refused.
    pub text: String,
    /// How many refusals before the hold lets go on its own. `None`
    /// holds until it is cleared, or until the session-wide escape
    /// hatch opens.
    pub times: Option<u32>,
    /// Refusals so far.
    pub refused: u32,
}

/// A file-safe form of a caller-supplied id.
///
/// An id reaches this from the command line, and it names a file in the
/// session directory. A `/` or a `..` in one would put that file
/// somewhere else entirely.
fn sanitize(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "id".to_string()
    } else {
        cleaned
    }
}

impl Store {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The state directory for one session.
    ///
    /// An invalid id is contained rather than trusted. gaff degrades; it
    /// never blocks a session over a bad id.
    fn session_dir(&self, session: &str) -> PathBuf {
        if valid_session_id(session) {
            self.root.join(session)
        } else {
            self.root.join("_invalid-session")
        }
    }

    fn ledger_path(&self, session: &str) -> PathBuf {
        self.session_dir(session).join("ledger.jsonl")
    }

    fn ledger_lines(&self, session: &str) -> Vec<Value> {
        let Ok(bytes) = std::fs::read_to_string(self.ledger_path(session)) else {
            return Vec::new();
        };
        bytes
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Run `body` while holding an exclusive lock on the session's
    /// ledger. Claude Code fires tool hooks in parallel, so several `gaff
    /// hook` processes race on one session. The lock serializes the
    /// read-count-append critical section, so no crossing is lost and no
    /// two writers interleave a line. The lock is a sidecar file, so the
    /// ledger's own bytes stay a clean append log.
    fn with_ledger_lock<T>(
        &self,
        session: &str,
        body: impl FnOnce() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        std::fs::create_dir_all(self.session_dir(session))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.session_dir(session).join("ledger.lock"))?;
        lock.lock_exclusive()?;
        let result = body();
        // Best-effort unlock; the lock also drops with the handle.
        let _ = fs2::FileExt::unlock(&lock);
        result
    }

    /// Append one line as a single write. Under `O_APPEND` a lone write
    /// of a short line is atomic, so a line never interleaves with a
    /// concurrent writer even outside the lock.
    fn append_ledger(&self, session: &str, line: &Value) -> std::io::Result<()> {
        std::fs::create_dir_all(self.session_dir(session))?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.ledger_path(session))?;
        f.write_all(format!("{line}\n").as_bytes())
    }

    /// Record a tool call. The `tool_use_id` deduplicates it, because
    /// `PreToolUse`, `PostToolUse`, and `PostToolUseFailure` carry the
    /// same id. One tool call counts once.
    ///
    /// Returns the new tool-call count. Returns `None` when this id was
    /// already counted. The count is exact under concurrent hooks,
    /// because the read and the append run under the ledger lock.
    pub fn record_tool_call(
        &self,
        session: &str,
        tool_use_id: &str,
    ) -> std::io::Result<Option<u64>> {
        self.with_ledger_lock(session, || {
            let lines = self.ledger_lines(session);
            let already = lines
                .iter()
                .any(|l| l["unit"] == "tool_calls" && l["id"] == tool_use_id);
            if already {
                return Ok(None);
            }
            let count = 1 + lines.iter().filter(|l| l["unit"] == "tool_calls").count() as u64;
            self.append_ledger(session, &json!({"id": tool_use_id, "unit": "tool_calls"}))?;
            Ok(Some(count))
        })
    }

    /// Record a user prompt. Returns the new prompt count. The count is
    /// exact under concurrent hooks; the read and append run under the
    /// ledger lock.
    pub fn record_prompt(&self, session: &str) -> std::io::Result<u64> {
        self.with_ledger_lock(session, || {
            let count = 1 + self
                .ledger_lines(session)
                .iter()
                .filter(|l| l["unit"] == "prompts")
                .count() as u64;
            self.append_ledger(session, &json!({"unit": "prompts"}))?;
            Ok(count)
        })
    }

    #[must_use]
    pub fn counts(&self, session: &str) -> Counts {
        let lines = self.ledger_lines(session);
        Counts {
            tool_calls: lines.iter().filter(|l| l["unit"] == "tool_calls").count() as u64,
            prompts: lines.iter().filter(|l| l["unit"] == "prompts").count() as u64,
        }
    }

    pub fn write_pending(&self, session: &str, name: &str, multiple: u64) -> std::io::Result<()> {
        std::fs::create_dir_all(self.session_dir(session))?;
        std::fs::write(
            self.session_dir(session).join(format!("pending-{name}")),
            format!("{multiple}\n"),
        )
    }

    #[must_use]
    pub fn pending_multiple(&self, session: &str, name: &str) -> Option<u64> {
        let path = self.session_dir(session).join(format!("pending-{name}"));
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }

    /// Consume a pending recurring entry. This deletes the pending file
    /// and records the flushed multiple as the cursor.
    pub fn consume_pending(&self, session: &str, name: &str, multiple: u64) -> std::io::Result<()> {
        std::fs::write(
            self.session_dir(session).join(format!("cursor-{name}")),
            format!("{multiple}\n"),
        )?;
        std::fs::remove_file(self.session_dir(session).join(format!("pending-{name}")))
    }

    pub fn write_oneshot(
        &self,
        session: &str,
        id: &str,
        after: u64,
        at: u64,
        text: &str,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(self.session_dir(session))?;
        // Reusing an id re-arms it. A stale fired marker from an earlier
        // one-shot with the same id would otherwise make the new one a
        // silent no-op.
        let id = sanitize(id);
        std::fs::remove_file(self.session_dir(session).join(format!("fired-{id}"))).ok();
        let line = serde_json::to_string(&json!({"after": after, "at": at, "text": text}))
            .unwrap_or_default();
        std::fs::write(
            self.session_dir(session).join(format!("oneshot-{id}.json")),
            format!("{line}\n"),
        )
    }

    /// The one-shots of a session, sorted by id. The sort keeps the
    /// merge order deterministic.
    #[must_use]
    pub fn oneshots(&self, session: &str) -> Vec<Oneshot> {
        let Ok(entries) = std::fs::read_dir(self.session_dir(session)) else {
            return Vec::new();
        };
        let mut out: Vec<Oneshot> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name
                    .strip_prefix("oneshot-")?
                    .strip_suffix(".json")?
                    .to_string();
                let v: Value =
                    serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
                Some(Oneshot {
                    id,
                    at: v["at"].as_u64()?,
                    text: v["text"].as_str()?.to_string(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// The names of all armed recurring entries, sorted.
    #[must_use]
    pub fn pendings(&self, session: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.session_dir(session)) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_prefix("pending-")
                    .map(ToString::to_string)
            })
            .collect();
        out.sort();
        out
    }

    #[must_use]
    pub fn is_fired(&self, session: &str, id: &str) -> bool {
        self.session_dir(session)
            .join(format!("fired-{id}"))
            .exists()
    }

    /// Claim a one-shot before it fires. The create uses `O_EXCL`, so
    /// exactly one racing invocation wins. Every other invocation gets
    /// `false`.
    #[must_use]
    pub fn claim_fired(&self, session: &str, id: &str) -> bool {
        std::fs::create_dir_all(self.session_dir(session)).is_ok()
            && OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(self.session_dir(session).join(format!("fired-{id}")))
                .is_ok()
    }

    /// Re-arm after a compaction. This deletes every fired marker, so a
    /// consumed one-shot becomes eligible again. The compaction erased
    /// the content of that one-shot from the context.
    pub fn clear_fired(&self, session: &str) {
        let Ok(entries) = std::fs::read_dir(self.session_dir(session)) else {
            return;
        };
        for e in entries.filter_map(Result::ok) {
            if e.file_name().to_string_lossy().starts_with("fired-") {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }

    /// The active profile for the session, if the session state names
    /// one. This is the authoritative record: it lives in user-level
    /// state, outside the repo tree.
    #[must_use]
    pub fn profile(&self, session: &str) -> Option<String> {
        std::fs::read_to_string(self.session_dir(session).join("profile"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Record the active profile and arm a re-prime. A switch that does
    /// not change the name arms nothing, so a repeated set is quiet.
    pub fn set_profile(&self, session: &str, name: &str) -> std::io::Result<bool> {
        if self.profile(session).as_deref() == Some(name) {
            return Ok(false);
        }
        std::fs::create_dir_all(self.session_dir(session))?;
        std::fs::write(self.session_dir(session).join("profile"), format!("{name}\n"))?;
        std::fs::write(self.session_dir(session).join("reprime"), "")?;
        Ok(true)
    }

    /// Consume the re-prime marker. A profile switch changes which
    /// sections apply, so the next flush must deliver them all rather
    /// than wait for each refresh cadence to come around.
    #[must_use]
    pub fn take_reprime(&self, session: &str) -> bool {
        std::fs::remove_file(self.session_dir(session).join("reprime")).is_ok()
    }

    /// Hold this session open until the agent says the work is done.
    ///
    /// The agent-facing stop hook. It is text and nothing else: gaff
    /// refuses the stop and delivers the text, and the model reading it
    /// decides whether the work is finished. No command runs, which is
    /// why an agent may set one at all — `gaff trust` exists precisely
    /// so an agent cannot schedule command execution for itself.
    ///
    /// # Errors
    /// Returns the IO error when the session directory cannot be
    /// written.
    pub fn write_hold(
        &self,
        session: &str,
        id: &str,
        text: &str,
        times: Option<u32>,
    ) -> std::io::Result<()> {
        let dir = self.session_dir(session).join("holds");
        std::fs::create_dir_all(&dir)?;
        let record = serde_json::json!({"text": text, "times": times, "refused": 0});
        std::fs::write(dir.join(sanitize(id)), record.to_string())
    }

    /// The stop holds this session set for itself, by id.
    #[must_use]
    pub fn holds(&self, session: &str) -> Vec<Hold> {
        let Ok(entries) = std::fs::read_dir(self.session_dir(session).join("holds")) else {
            return Vec::new();
        };
        let mut out: Vec<Hold> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let id = e.file_name().to_string_lossy().to_string();
                let raw = std::fs::read_to_string(e.path()).ok()?;
                let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
                Some(Hold {
                    text: v.get("text")?.as_str()?.to_string(),
                    times: v
                        .get("times")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok()),
                    refused: v
                        .get("refused")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0),
                    id,
                })
            })
            .collect();
        // A stable order, so the same session reports the same first
        // hold every time rather than whatever the directory yields.
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Count one refusal against a hold, and say whether it has spent
    /// its budget. A hold with no budget never spends it.
    pub fn refuse_hold(&self, session: &str, id: &str) -> bool {
        let path = self.session_dir(session).join("holds").join(sanitize(id));
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return false;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return false;
        };
        let refused = v.get("refused").and_then(serde_json::Value::as_u64).unwrap_or(0) + 1;
        v["refused"] = serde_json::json!(refused);
        let _ = std::fs::write(&path, v.to_string());
        // Spent once the budget is exceeded, not met. `--times 3` means
        // three refusals happen and the fourth stop goes through.
        v.get("times")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|budget| refused > budget)
    }

    /// Release a stop hold by id. `None` releases every one.
    pub fn clear_hold(&self, session: &str, id: Option<&str>) {
        let dir = self.session_dir(session).join("holds");
        match id {
            Some(id) => {
                let _ = std::fs::remove_file(dir.join(sanitize(id)));
            }
            None => {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    /// How many times in a row a goal has refused a stop, after    /// How many times in a row a goal has refused a stop, after
    /// counting this one.
    ///
    /// A goal that can never be met would otherwise refuse every stop
    /// forever, and there is no way out of that from inside the
    /// session. The count is the escape hatch.
    #[must_use]
    pub fn record_stop_refusal(&self, session: &str) -> u32 {
        let path = self.session_dir(session).join("stop-refusals");
        let next = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        if std::fs::create_dir_all(self.session_dir(session)).is_ok() {
            let _ = std::fs::write(&path, next.to_string());
        }
        next
    }

    /// Forget the refusal streak. Any stop that gaff allows ends it.
    pub fn clear_stop_refusals(&self, session: &str) {
        let _ = std::fs::remove_file(self.session_dir(session).join("stop-refusals"));
    }

    /// Append one line to the injection log. The log is the audit trail
    /// for what gaff put into a session, and it never blocks a flush:
    /// an unwritable log is silent.
    pub fn record_injection(&self, session: &str, event: &str, text: &str) {
        let Ok(()) = std::fs::create_dir_all(self.session_dir(session)) else {
            return;
        };
        let names: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("[gaff:"))
            .filter_map(|l| l.split(']').next())
            .collect();
        let line = serde_json::to_string(&json!({
            "event": event,
            "bytes": text.len(),
            "entries": names,
        }))
        .unwrap_or_default();
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.session_dir(session).join("injections.jsonl"))
        {
            let _ = f.write_all(format!("{line}\n").as_bytes());
        }
    }

    /// Read the injection log, oldest first.
    #[must_use]
    pub fn injections(&self, session: &str) -> Vec<Value> {
        std::fs::read_to_string(self.session_dir(session).join("injections.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Every session directory under the root, sorted.
    #[must_use]
    pub fn sessions(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.root.join("degraded").exists()
    }

    /// Write the loud-degradation marker. The config failed to parse in
    /// this session.
    pub fn mark_degraded(&self) {
        if std::fs::create_dir_all(&self.root).is_ok() {
            std::fs::write(self.root.join("degraded"), "").ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> Store {
        let root =
            std::env::temp_dir().join(format!("gaff-state-test-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        Store::new(root)
    }

    #[test]
    fn resolve_prefers_explicit_dir_and_joins_relative() {
        let cwd = Path::new("/work/repo");
        assert_eq!(
            resolve_root(Some(".gaff-state"), None, None, cwd),
            Some(PathBuf::from("/work/repo/.gaff-state"))
        );
        assert_eq!(
            resolve_root(Some("/abs/state"), Some("/xdg"), Some("/home/u"), cwd),
            Some(PathBuf::from("/abs/state"))
        );
    }

    #[test]
    fn resolve_falls_back_user_scoped_then_none() {
        let cwd = Path::new("/work/repo");
        let xdg = resolve_root(None, Some("/xdg"), Some("/home/u"), cwd).unwrap();
        assert!(xdg.starts_with("/xdg/gaff/"), "{xdg:?}");
        let home = resolve_root(None, None, Some("/home/u"), cwd).unwrap();
        assert!(home.starts_with("/home/u/.local/state/gaff/"), "{home:?}");
        assert_eq!(resolve_root(None, None, None, cwd), None);
    }

    #[test]
    fn cwd_key_is_stable() {
        assert_eq!(cwd_key(Path::new("/a")), cwd_key(Path::new("/a")));
        assert_ne!(cwd_key(Path::new("/a")), cwd_key(Path::new("/b")));
    }

    #[test]
    fn tool_calls_dedupe_on_id() {
        let s = temp_store("dedupe");
        assert_eq!(s.record_tool_call("s", "t1").unwrap(), Some(1));
        assert_eq!(s.record_tool_call("s", "t1").unwrap(), None);
        assert_eq!(s.record_tool_call("s", "t2").unwrap(), Some(2));
        assert_eq!(s.counts("s").tool_calls, 2);
    }

    #[test]
    fn absent_state_reads_as_zero() {
        let s = temp_store("absent");
        assert_eq!(s.counts("nope"), Counts::default());
        assert!(s.oneshots("nope").is_empty());
        assert!(!s.is_fired("nope", "x"));
    }

    #[test]
    fn concurrent_tool_calls_lose_no_cadence_crossing() {
        // 40 distinct ids recorded from 8 threads. Every count from 1..=40
        // must appear exactly once, so no crossing (e.g. every 20th) is
        // lost or duplicated.
        let s = temp_store("concurrent");
        let root = s.root;
        let seen: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for t in 0..8 {
                let root = root.clone();
                let seen = &seen;
                scope.spawn(move || {
                    for i in 0..5 {
                        let id = format!("t{}", t * 5 + i);
                        let store = Store::new(root.clone());
                        if let Ok(Some(count)) = store.record_tool_call("s", &id) {
                            seen.lock().unwrap().push(count);
                        }
                    }
                });
            }
        });
        let mut counts = seen.into_inner().unwrap();
        counts.sort_unstable();
        assert_eq!(
            counts,
            (1..=40).collect::<Vec<_>>(),
            "each count exactly once"
        );
    }

    #[test]
    fn claim_fired_exactly_once_under_races() {
        let s = temp_store("claim");
        let root = s.root;
        let wins: usize = std::thread::scope(|scope| {
            // Spawn every racer before the first join, so the claims
            // actually race.
            let mut handles = Vec::new();
            for _ in 0..16 {
                let root = root.clone();
                handles.push(
                    scope.spawn(move || usize::from(Store::new(root).claim_fired("s", "race"))),
                );
            }
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        });
        assert_eq!(wins, 1, "exactly one racer may claim the marker");
    }

    #[test]
    fn clear_fired_reenables_claim() {
        let s = temp_store("rearm");
        assert!(s.claim_fired("s", "ci"));
        assert!(!s.claim_fired("s", "ci"));
        s.clear_fired("s");
        assert!(s.claim_fired("s", "ci"));
    }

    #[test]
    fn rewriting_a_oneshot_rearms_a_fired_id() {
        let s = temp_store("reuse");
        s.write_oneshot("s", "ci", 0, 0, "first").unwrap();
        assert!(s.claim_fired("s", "ci"), "fires the first time");
        assert!(!s.claim_fired("s", "ci"), "does not fire twice");
        // Scheduling the id again must clear the stale fired marker.
        s.write_oneshot("s", "ci", 0, 0, "second").unwrap();
        assert!(s.claim_fired("s", "ci"), "the reused id fires again");
    }
}

#[cfg(test)]
mod profile_state_tests {
    use super::*;

    fn store(tag: &str) -> Store {
        let d = std::env::temp_dir().join(format!("gaff-ps-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        Store::new(d)
    }

    #[test]
    fn setting_a_profile_arms_one_reprime() {
        let s = store("set");
        assert!(s.profile("x").is_none(), "absent state reads as no profile");
        assert!(s.set_profile("x", "focus").unwrap(), "the switch changed it");
        assert_eq!(s.profile("x").as_deref(), Some("focus"));
        assert!(s.take_reprime("x"), "a switch arms a re-prime");
        assert!(!s.take_reprime("x"), "the marker is consumed once");

        assert!(
            !s.set_profile("x", "focus").unwrap(),
            "a repeated set changes nothing"
        );
        assert!(!s.take_reprime("x"), "a no-op set arms no re-prime");
    }

    #[test]
    fn the_injection_log_records_the_entry_names() {
        let s = store("log");
        assert!(s.injections("x").is_empty());
        s.record_injection("x", "PostToolBatch", "[gaff:build-clean] keep it green");
        s.record_injection("x", "SessionStart", "[gaff:prime]\nbody\n\n[gaff:skills] more");
        let lines = s.injections("x");
        assert_eq!(lines.len(), 2, "one line per injection, in order");
        assert_eq!(lines[0]["event"], "PostToolBatch");
        assert_eq!(lines[0]["entries"][0], "build-clean");
        assert_eq!(lines[1]["entries"][0], "prime");
        assert_eq!(lines[1]["entries"][1], "skills");
        assert!(lines[0]["bytes"].as_u64().unwrap() > 0);
    }
}

#[cfg(test)]
mod session_id_tests {
    use super::*;

    #[test]
    fn a_traversing_session_id_never_escapes_the_root() {
        let root = std::env::temp_dir().join(format!("gaff-sid-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::new(root.clone());
        store.record_prompt("../escaped").unwrap();
        assert!(
            !root.parent().unwrap().join("escaped").exists(),
            "the id must not escape the state root"
        );
        assert!(root.join("_invalid-session").is_dir(), "it is contained");
    }

    #[test]
    fn the_id_rules_accept_real_ids_and_refuse_separators() {
        assert!(valid_session_id("abc-123_XY.z"));
        assert!(!valid_session_id("../x"));
        assert!(!valid_session_id("a/b"));
        assert!(!valid_session_id(".."));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id(&"x".repeat(129)));
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;

    fn store(tag: &str) -> Store {
        let d = std::env::temp_dir().join(format!("gaff-hold-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        Store::new(d)
    }

    #[test]
    fn a_hold_survives_until_it_is_released() {
        let s = store("basic");
        s.write_hold("sess", "intent", "finish the work", None).unwrap();
        let held = s.holds("sess");
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].id, "intent");
        assert_eq!(held[0].text, "finish the work");
        assert_eq!(held[0].times, None);
        s.clear_hold("sess", Some("intent"));
        assert!(s.holds("sess").is_empty());
    }

    #[test]
    fn several_holds_report_in_a_stable_order() {
        // The same session must name the same first hold every time,
        // rather than whatever the directory happens to yield.
        let s = store("many");
        s.write_hold("sess", "b", "second", None).unwrap();
        s.write_hold("sess", "a", "first", None).unwrap();
        let names: Vec<String> = s.holds("sess").into_iter().map(|h| h.id).collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn rewriting_an_id_replaces_rather_than_stacks() {
        let s = store("rewrite");
        s.write_hold("sess", "intent", "old", None).unwrap();
        s.write_hold("sess", "intent", "new", None).unwrap();
        assert_eq!(s.holds("sess").len(), 1);
        assert_eq!(s.holds("sess")[0].text, "new");
    }

    #[test]
    fn a_budgeted_hold_spends_itself() {
        // "Push back this many times, then let me stop."
        let s = store("budget");
        s.write_hold("sess", "nudge", "are you sure", Some(3)).unwrap();
        assert!(!s.refuse_hold("sess", "nudge"), "refusal 1 of 3");
        assert!(!s.refuse_hold("sess", "nudge"), "refusal 2 of 3");
        assert!(!s.refuse_hold("sess", "nudge"), "refusal 3 of 3");
        assert!(s.refuse_hold("sess", "nudge"), "the fourth stop goes through");
    }

    #[test]
    fn an_unbudgeted_hold_never_spends_itself() {
        let s = store("forever");
        s.write_hold("sess", "hard", "not done", None).unwrap();
        for _ in 0..50 {
            assert!(!s.refuse_hold("sess", "hard"));
        }
    }

    #[test]
    fn an_id_cannot_escape_the_session_directory() {
        // An id arrives from the command line and names a file. A `/`
        // or a `..` in one would put that file somewhere else.
        let s = store("escape");
        s.write_hold("sess", "../../etc/passwd", "nope", None).unwrap();
        let held = s.holds("sess");
        assert_eq!(held.len(), 1);
        assert!(!held[0].id.contains('/'), "{:?}", held[0].id);
        assert!(!held[0].id.contains(".."), "{:?}", held[0].id);
    }

    #[test]
    fn the_refusal_streak_counts_and_clears() {
        // A hold nothing clears would end the session's ability to end.
        let s = store("streak");
        assert_eq!(s.record_stop_refusal("sess"), 1);
        assert_eq!(s.record_stop_refusal("sess"), 2);
        s.clear_stop_refusals("sess");
        assert_eq!(s.record_stop_refusal("sess"), 1);
    }
}
