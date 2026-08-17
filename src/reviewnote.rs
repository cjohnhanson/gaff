//! The review-note check a merge gate runs.
//!
//! A push carries a note under `refs/notes/reviews` on each pushed
//! tip. That note records the reviews the change passed. The reviews a
//! change must pass are declared in `.gaff/gaff.yml`, so this module
//! never hard-codes a review name: it asks the config, then reads the
//! note for each declared name.
//!
//! This lived as a shell script in each consuming repository. Six
//! copies of branching security logic in a language with no test
//! framework is the wrong home for it, and testing it from the
//! consuming crate's own test suite was worse. It is Rust here, tested
//! next to the code.

use std::process::Command;

/// One pushed ref, as git writes it to a pre-push hook's stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedRef {
    pub local_sha: String,
    pub remote_ref: String,
}

/// The all-zero sha git sends for a deletion.
const ZERO: &str = "0000000000000000000000000000000000000000";

/// Parse git's pre-push lines: `<local-ref> <local-sha> <remote-ref> <remote-sha>`.
///
/// A deletion merges nothing, so it is dropped. A push of the notes ref
/// shares review records rather than proposing a change, so it is
/// dropped too. The exemption keys on the remote ref: a notes object
/// pushed at a branch lands on that branch and is checked.
#[must_use]
pub fn parse_refs(stdin: &str) -> Vec<PushedRef> {
    stdin
        .lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let (_local_ref, local_sha, remote_ref) = (f.next()?, f.next()?, f.next()?);
            if local_sha == ZERO || remote_ref.starts_with("refs/notes/") {
                return None;
            }
            Some(PushedRef {
                local_sha: local_sha.to_string(),
                remote_ref: remote_ref.to_string(),
            })
        })
        .collect()
}

/// Why a tip failed the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// No note at all on the commit.
    Note,
    /// A note exists and does not name these declared reviews.
    Reviews(Vec<String>),
}

/// Check one note body against the declared review names.
///
/// The match is case-insensitive and by substring, because a note is
/// prose. `None` means the tip passes.
#[must_use]
pub fn shortfall(note: Option<&str>, required: &[String]) -> Option<Missing> {
    let Some(note) = note else {
        return Some(Missing::Note);
    };
    let lower = note.to_lowercase();
    let absent: Vec<String> = required
        .iter()
        .filter(|name| !lower.contains(&name.to_lowercase()))
        .cloned()
        .collect();
    if absent.is_empty() {
        None
    } else {
        Some(Missing::Reviews(absent))
    }
}

/// The commit a pull request proposes, read from the event payload.
///
/// A pull request event checks out a merge commit the forge creates.
/// No reviewer saw that commit, so checking it would refuse every pull
/// request. The branch head is what a reviewer read.
///
/// Both variables come from the runner. A local shell that sets one
/// still faces the ordinary check, so a single variable cannot turn
/// the review requirement off.
#[must_use]
pub fn pull_request_head(env: &dyn Fn(&str) -> Option<String>) -> Option<Result<String, String>> {
    if env("GITHUB_ACTIONS").as_deref() != Some("true")
        || env("GITHUB_EVENT_NAME").as_deref() != Some("pull_request")
    {
        return None;
    }
    let Some(path) = env("GITHUB_EVENT_PATH") else {
        return Some(Err("GITHUB_EVENT_PATH is unset".to_string()));
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Some(Err(format!("{path} could not be read")));
    };
    Some(head_sha_from_event(&body).ok_or_else(|| format!("{path} names no pull request head sha")))
}

/// Pull `.pull_request.head.sha` out of an event payload.
///
/// A payload holds many sha fields, so the first one found is the
/// wrong answer. This walks to the head object before reading.
#[must_use]
pub fn head_sha_from_event(body: &str) -> Option<String> {
    let pr = body.find("\"pull_request\"")?;
    let head = body[pr..].find("\"head\"")? + pr;
    let sha_key = body[head..].find("\"sha\"")? + head;
    let rest = &body[sha_key + 5..];
    let open = rest.find('"')? + 1;
    let close = rest[open..].find('"')? + open;
    let sha = &rest[open..close];
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(sha.to_string())
    } else {
        None
    }
}

/// Read the review note on a commit. `None` when the commit has none.
fn note_body(cwd: &std::path::Path, sha: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["notes", "--ref=reviews", "show", sha])
        .current_dir(cwd)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

/// Resolve a pushed sha to the commit it names. A tag peels to one.
fn peel(cwd: &std::path::Path, sha: &str) -> String {
    Command::new("git")
        .args([
            "rev-parse",
            "--quiet",
            "--verify",
            &format!("{sha}^{{commit}}"),
        ])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sha.to_string())
}

/// What the check decided about one tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub commit: String,
    pub remote_ref: String,
    pub missing: Option<Missing>,
}

/// Check every pushed tip against the declared reviews.
///
/// `head_override` replaces the pushed refs when a pull request event
/// supplies one, because the checked-out merge commit is not what a
/// reviewer read.
#[must_use]
pub fn check(
    cwd: &std::path::Path,
    refs: &[PushedRef],
    required: &[String],
    head_override: Option<&str>,
) -> Vec<Verdict> {
    let targets: Vec<PushedRef> = head_override.map_or_else(
        || refs.to_vec(),
        |sha| {
            vec![PushedRef {
                local_sha: sha.to_string(),
                remote_ref: "refs/heads/pull-request".to_string(),
            }]
        },
    );
    targets
        .iter()
        .map(|r| {
            let commit = peel(cwd, &r.local_sha);
            let note = note_body(cwd, &commit);
            Verdict {
                missing: shortfall(note.as_deref(), required),
                commit,
                remote_ref: r.remote_ref.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_deletion_is_not_checked() {
        let line = format!("refs/heads/x {ZERO} refs/heads/main abc\n");
        assert!(parse_refs(&line).is_empty());
    }

    #[test]
    fn a_notes_ref_push_is_not_checked() {
        let line = "refs/notes/reviews abc123 refs/notes/reviews def456\n";
        assert!(parse_refs(line).is_empty());
    }

    #[test]
    fn a_notes_object_pushed_at_a_branch_is_checked() {
        // It lands on that branch, so it proposes a change.
        let line = "refs/notes/reviews abc123 refs/heads/main def456\n";
        assert_eq!(parse_refs(line).len(), 1);
    }

    #[test]
    fn an_ordinary_push_is_checked() {
        let line = "refs/heads/topic abc123 refs/heads/main def456\n";
        let refs = parse_refs(line);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].local_sha, "abc123");
        assert_eq!(refs[0].remote_ref, "refs/heads/main");
    }

    #[test]
    fn a_missing_note_is_a_shortfall() {
        assert_eq!(
            shortfall(None, &req(["fresh-eyes"].as_slice())),
            Some(Missing::Note)
        );
    }

    #[test]
    fn a_note_naming_every_review_passes() {
        let note = "fresh-eyes: a reviewer read it. Mutation: guard removed, test red.";
        assert_eq!(
            shortfall(Some(note), &req(&["fresh-eyes", "mutation"])),
            None
        );
    }

    #[test]
    fn a_note_missing_one_review_names_it() {
        let note = "fresh-eyes: a reviewer read it.";
        assert_eq!(
            shortfall(Some(note), &req(&["fresh-eyes", "mutation"])),
            Some(Missing::Reviews(req(&["mutation"])))
        );
    }

    #[test]
    fn an_empty_policy_accepts_any_note() {
        // `reviews: []` states that an author chose no required review.
        assert_eq!(shortfall(Some("anything"), &[]), None);
    }

    #[test]
    fn an_empty_policy_still_requires_a_note() {
        assert_eq!(shortfall(None, &[]), Some(Missing::Note));
    }

    #[test]
    fn the_head_sha_comes_from_the_head_object_not_the_first_sha() {
        // A real payload carries a base sha before the head sha.
        let body = r#"{"pull_request":{"base":{"sha":"1111111111111111111111111111111111111111"},"head":{"sha":"2222222222222222222222222222222222222222"}}}"#;
        assert_eq!(
            head_sha_from_event(body).as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn a_payload_with_no_head_yields_nothing() {
        assert_eq!(head_sha_from_event(r#"{"pull_request":{}}"#), None);
    }

    #[test]
    fn a_short_sha_is_refused() {
        let body = r#"{"pull_request":{"head":{"sha":"abc"}}}"#;
        assert_eq!(head_sha_from_event(body), None);
    }

    #[test]
    fn a_local_shell_setting_the_event_name_is_not_a_pull_request() {
        let env = |k: &str| match k {
            "GITHUB_EVENT_NAME" => Some("pull_request".to_string()),
            _ => None,
        };
        assert!(pull_request_head(&env).is_none());
    }

    #[test]
    fn a_runner_without_the_event_name_is_not_a_pull_request() {
        let env = |k: &str| match k {
            "GITHUB_ACTIONS" => Some("true".to_string()),
            _ => None,
        };
        assert!(pull_request_head(&env).is_none());
    }

    #[test]
    fn a_runner_on_a_pull_request_with_no_payload_path_errors() {
        let env = |k: &str| match k {
            "GITHUB_ACTIONS" => Some("true".to_string()),
            "GITHUB_EVENT_NAME" => Some("pull_request".to_string()),
            _ => None,
        };
        assert!(matches!(pull_request_head(&env), Some(Err(_))));
    }
}
