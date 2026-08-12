//! Session state: append-only ledger, pending/cursor files, one-shots,
//! and fired markers.
//!
//! Layout (flat — empty directories are unrepresentable in fixtures and
//! participate in byte diffs):
//!
//! ```text
//! <root>/degraded                    marker: config failed to parse
//! <root>/<session>/ledger.jsonl      one line per counted event
//! <root>/<session>/pending-<name>    recurring reminder armed (multiple)
//! <root>/<session>/cursor-<name>     last flushed multiple
//! <root>/<session>/oneshot-<id>.json scheduled one-shot
//! <root>/<session>/fired-<id>        one-shot consumed (O_EXCL claim)
//! ```
//!
//! The root resolves from `GAFF_STATE_DIR` (relative paths join the
//! process cwd) or falls back to a user-scoped path keyed by cwd —
//! never inside the repo, where `git clean -xdf` would erase it
//! mid-session. Absent state always reads as zero, never as an error.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

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

/// Resolve the state root from explicit inputs. Pure so it can be tested
/// without mutating the process environment.
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

/// Stable key for a working directory (FNV-1a, not `DefaultHasher`,
/// which is unstable across Rust releases).
fn cwd_key(cwd: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in cwd.to_string_lossy().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Store {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn session_dir(&self, session: &str) -> PathBuf {
        self.root.join(session)
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

    fn append_ledger(&self, session: &str, line: &Value) -> std::io::Result<()> {
        std::fs::create_dir_all(self.session_dir(session))?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.ledger_path(session))?;
        writeln!(f, "{line}")
    }

    /// Record a tool call, deduplicating on `tool_use_id` (`PreToolUse`,
    /// `PostToolUse`, and `PostToolUseFailure` all carry the same id; the
    /// call counts once). Returns the new tool-call count, or `None` if
    /// this id was already counted.
    pub fn record_tool_call(
        &self,
        session: &str,
        tool_use_id: &str,
    ) -> std::io::Result<Option<u64>> {
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
    }

    /// Record a user prompt. Returns the new prompt count.
    pub fn record_prompt(&self, session: &str) -> std::io::Result<u64> {
        let count = 1 + self
            .ledger_lines(session)
            .iter()
            .filter(|l| l["unit"] == "prompts")
            .count() as u64;
        self.append_ledger(session, &json!({"unit": "prompts"}))?;
        Ok(count)
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

    /// Consume a pending recurring reminder: delete the pending file and
    /// record the flushed multiple as the cursor.
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
        let line = serde_json::to_string(&json!({"after": after, "at": at, "text": text}))
            .unwrap_or_default();
        std::fs::write(
            self.session_dir(session).join(format!("oneshot-{id}.json")),
            format!("{line}\n"),
        )
    }

    /// One-shots for a session, sorted by id for deterministic merge order.
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

    /// Names of all armed (pending) recurring entries, sorted.
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

    /// Claim a one-shot for firing. `O_EXCL` create: exactly one racing
    /// invocation wins; everyone else sees `false`.
    #[must_use]
    pub fn claim_fired(&self, session: &str, id: &str) -> bool {
        std::fs::create_dir_all(self.session_dir(session)).is_ok()
            && OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(self.session_dir(session).join(format!("fired-{id}")))
                .is_ok()
    }

    /// Compaction re-arm: delete every fired marker so consumed one-shots
    /// become eligible again (their content was erased from context).
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

    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.root.join("degraded").exists()
    }

    /// Loud-degradation marker: config failed to parse this session.
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
    fn claim_fired_exactly_once_under_races() {
        let s = temp_store("claim");
        let root = s.root.clone();
        let wins: usize = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let root = root.clone();
                    scope.spawn(move || usize::from(Store::new(root).claim_fired("s", "race")))
                })
                .collect();
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
}
