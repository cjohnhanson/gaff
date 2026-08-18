//! The review-note check a merge gate runs.
//!
//! A push carries a note under `refs/notes/reviews` on each pushed
//! tip. That note records the reviews a change passed.
//!
//! `.gaff/gaff.yml` declares the reviews a change must pass. This
//! module reads that list. It then reads the note on each pushed tip
//! and looks for a sign-off naming every declared review.
//!
//! This check ran as a shell script in six repositories before gaff
//! took it. See commit efbfdc4.

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
/// Two kinds of line are dropped. A deletion merges nothing. A push of
/// the notes ref shares review records and proposes no change.
///
/// The decision reads the remote ref, not the local one. A notes
/// object pushed at a branch lands on that branch, so it is checked.
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

/// Count the lines that carry text but do not carry a ref.
///
/// git writes four fields to a pre-push hook. A line with fewer is a
/// truncated stream, not a push.
///
/// This exists because dropping such a line silently defeated the
/// guard that reads an empty ref list as a wiring fault. One line
/// reading `refs/heads/main` parsed to no refs and to non-empty
/// stdin, so the check reported "nothing to check" and exited 0. A
/// caller that lost most of the stream then passed the gate.
#[must_use]
pub fn truncated_lines(stdin: &str) -> usize {
    stdin
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| line.split_whitespace().count() < 4)
        .count()
}

/// Why a tip failed the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// No note at all on the commit.
    Note,
    /// No sign-off line for these declared reviews.
    Reviews(Vec<String>),
    /// A review signed off as failed. Naming it is the point.
    Failed(Vec<String>),
    /// A sign-off names a different commit than the one being pushed.
    WrongCommit { review: String, named: String },
    /// Two sign-off lines for one review. Which one counts is unclear,
    /// so neither does.
    Duplicate(String),
    /// Evidence under the floor. A script cannot grade evidence. It can
    /// refuse a keystroke.
    ThinEvidence { review: String, words: usize },
    /// A sign-off names too short a sha, or one that is not hex. A
    /// prefix match with no floor accepts a single character, and that
    /// matches one commit in sixteen.
    ShortCommit { review: String, named: String },
}

/// The fewest words of evidence a sign-off may carry.
///
/// A floor, not a judgment. A script cannot tell a real reason from a
/// plausible one, so it refuses only the case where nobody tried.
///
/// A test reads this constant and the man page together, so the two
/// cannot disagree. The page once stated a floor that no test read,
/// and an edit could contradict the code with every test green.
pub const EVIDENCE_FLOOR: usize = 3;

/// One parsed sign-off line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signoff {
    pub review: String,
    pub passed: bool,
    pub commit: String,
    pub evidence_words: usize,
}

/// Read the sign-off lines out of a note body.
///
/// The form is one line, anchored at the start of a line:
///
/// ```text
/// signoff[fresh-eyes] PASS 4f1c2ab removed the guard and walk_up went red
/// ```
///
/// Prose around the lines is ignored, so a note can carry a narrative.
/// A line that does not parse is not a sign-off and is ignored, which
/// leaves its review unsigned rather than silently accepted.
#[must_use]
pub fn parse_signoffs(note: &str) -> Vec<Signoff> {
    note.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("signoff[")?;
            let close = rest.find(']')?;
            let review = rest[..close].to_string();
            if review.is_empty() {
                return None;
            }
            let mut f = rest[close + 1..].split_whitespace();
            let verdict = f.next()?;
            let passed = match verdict {
                "PASS" => true,
                "FAIL" => false,
                _ => return None,
            };
            let commit = f.next()?.to_string();
            Some(Signoff {
                review,
                passed,
                commit,
                evidence_words: f.count(),
            })
        })
        .collect()
}

/// Check a note against the declared reviews, for one commit.
///
/// Every declared review needs one sign-off line that passed, names
/// this commit, and carries evidence.
///
/// Substring matching was the first design and it is unsound. A note
/// reading `mutation: skipped this round` contains `mutation`, and a
/// note reading `fresh-eyes: FAILED, do not merge` contains
/// `fresh-eyes`. Prose about a review reads exactly like a record of
/// one, so the line form carries a verdict instead.
///
/// The commit binding matters most. Without it a sign-off copies
/// forward onto a later commit nobody read, and nothing says so.
///
/// An empty policy passes every tip. A repo writing `reviews: []` has
/// stated that it requires no review. An absent declaration never
/// reaches here: the caller refuses it, because no policy must not
/// read as no review required.
#[must_use]
pub fn shortfall(note: Option<&str>, required: &[String], commit: &str) -> Vec<Missing> {
    if required.is_empty() {
        return Vec::new();
    }
    let Some(note) = note else {
        return vec![Missing::Note];
    };
    let signoffs = parse_signoffs(note);
    let mut faults = Vec::new();
    let mut unsigned = Vec::new();

    for name in required {
        let mine: Vec<&Signoff> = signoffs.iter().filter(|s| &s.review == name).collect();
        match mine.len() {
            0 => {
                unsigned.push(name.clone());
                continue;
            }
            1 => {}
            _ => {
                faults.push(Missing::Duplicate(name.clone()));
                continue;
            }
        }
        let s = mine[0];
        if !s.passed {
            faults.push(Missing::Failed(vec![name.clone()]));
            continue;
        }
        // A reviewer writes the short sha git prints, so a prefix
        // matches. The floor is what makes a prefix mean anything: one
        // hex character matches one commit in sixteen, so a sign-off
        // reading `PASS 5` signed off any commit starting with `5`.
        if !is_hex_sha(&s.commit) {
            faults.push(Missing::ShortCommit {
                review: name.clone(),
                named: s.commit.clone(),
            });
            continue;
        }
        // git accepts an uppercase abbreviation, so a reviewer who
        // pasted one names the right commit. A case-sensitive compare
        // reported it as a different commit and sent the reader
        // hunting for a commit that does not exist.
        if !commit
            .to_ascii_lowercase()
            .starts_with(&s.commit.to_ascii_lowercase())
        {
            faults.push(Missing::WrongCommit {
                review: name.clone(),
                named: s.commit.clone(),
            });
            continue;
        }
        if s.evidence_words < EVIDENCE_FLOOR {
            faults.push(Missing::ThinEvidence {
                review: name.clone(),
                words: s.evidence_words,
            });
        }
    }

    // A FAIL on a review the policy does not require still refuses the
    // push. A reviewer who wrote one meant it.
    let mut failed_extra: Vec<String> = signoffs
        .iter()
        .filter(|s| !s.passed && !required.contains(&s.review))
        .map(|s| s.review.clone())
        .collect();
    failed_extra.sort();
    failed_extra.dedup();
    if !failed_extra.is_empty() {
        faults.push(Missing::Failed(failed_extra));
    }

    if !unsigned.is_empty() {
        faults.insert(0, Missing::Reviews(unsigned));
    }
    faults
}

/// The commit a pull request proposes, read from the event payload.
///
/// On a pull request, GitHub creates a merge commit and the runner
/// checks that commit out. No reviewer read it, so a check against it
/// would refuse every pull request. A reviewer reads the branch head.
///
/// THE ENVIRONMENT REACHES THIS, and the caller decides what that
/// costs. `GITHUB_ACTIONS`, `GITHUB_EVENT_NAME` and
/// `GITHUB_EVENT_PATH` are ordinary variables, and nothing here tells
/// a runner from a shell. Setting all three and writing a payload
/// file makes this function name any commit the caller likes.
///
/// `gaff reviews check` does not call this, so a push cannot be
/// steered that way. `gaff ci` does, and `gaff ci` pushes nothing, so
/// the cost of a forged payload is a local run that certifies the
/// wrong commit. In a runner the variables come from the runner.
///
/// An earlier version wired this into the push path, and a reviewer
/// pushed an unreviewed commit past the gate with it against a bare
/// remote. The lesson kept is narrow: the push path reads no
/// environment. This function is not the push path.
///
/// Only `gaff ci` calls this. It substitutes the sha into the ref line
/// it synthesizes on the pre-push hook's stdin, so the check reads the
/// same one line per ref that git sends. One mechanism carries the
/// head, and `reviews check` takes no flag that names a commit. A flag
/// would be a second mechanism, and it also had to suppress the
/// empty-stdin guard to work.
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
/// Parsed, not searched. String searching walked to the first `"sha"`
/// after the first `"head"`, which is a different key whenever the
/// payload nests one, and fell through to `base.sha` when `head.sha`
/// was absent. Each wrong answer names a commit that is already
/// reviewed, so the check passed.
#[must_use]
pub fn head_sha_from_event(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let sha = v.get("pull_request")?.get("head")?.get("sha")?.as_str()?;
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(sha.to_string())
    } else {
        None
    }
}

/// Where a note for `sha` can sit in the notes tree.
///
/// git splits the notes tree into fanout directories once it grows,
/// and it rewrites the layout as the tree changes. A note written flat
/// today sits under `ab/cdef...` tomorrow, with no change to the
/// commit it annotates. The lookup tries every depth, shallowest first.
///
/// Two hex characters make each level, which is git's own split.
///
/// A sha that is not hex gets the flat candidate and no fanout. Every
/// candidate is then absent from the tree, so the tip is refused. An
/// earlier version sliced by byte index without checking, and a
/// multi-byte character on stdin panicked the gate.
fn note_paths(sha: &str) -> Vec<String> {
    let mut out = vec![sha.to_string()];
    if !is_hex_sha(sha) {
        return out;
    }
    for depth in 1..=3 {
        let cut = depth * 2;
        if sha.len() <= cut {
            break;
        }
        let mut path = String::with_capacity(sha.len() + depth);
        for level in 0..depth {
            path.push_str(&sha[level * 2..level * 2 + 2]);
            path.push('/');
        }
        path.push_str(&sha[cut..]);
        out.push(path);
    }
    out
}

/// True for a hex string long enough to name a commit.
///
/// git's own abbreviation floor is four characters. This one takes
/// seven, which is what git prints and what a reviewer copies. The
/// ceiling covers a sha256 repository, whose object names run to 64.
#[must_use]
pub fn is_hex_sha(s: &str) -> bool {
    s.len() >= SHA_FLOOR && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The first `SHA_FLOOR` characters, for a message.
///
/// Truncates on a character boundary, not a byte index. A sha reaching
/// a message is not always hex: it arrives on stdin, and a message
/// naming a bad value is exactly when a reader needs it. Slicing by
/// byte panicked the gate when a multi-byte character sat across the
/// cut.
#[must_use]
pub fn short(sha: &str) -> &str {
    match sha.char_indices().nth(SHA_FLOOR) {
        Some((byte, _)) => &sha[..byte],
        None => sha,
    }
}

/// The fewest characters a sign-off's commit may carry.
///
/// A one-character sha passed every check, because a prefix match with
/// no floor accepts it. `signoff[fresh-eyes] PASS 5 ...` then signed
/// off every commit whose sha starts with `5`, which is the
/// carry-forward the commit binding exists to stop.
pub const SHA_FLOOR: usize = 7;

/// Read the review note on a commit. `None` when the commit has none.
///
/// Reads the object database rather than spawning git. `refs/notes/
/// reviews` peels to a commit, whose tree holds one blob per annotated
/// commit, named by that commit's sha under a fanout.
fn note_body(cwd: &std::path::Path, sha: &str) -> Option<String> {
    let repo = gix::discover(cwd).ok()?;
    let mut reference = repo.find_reference(NOTES_REF).ok()?;
    let tree = reference
        .peel_to_id()
        .ok()?
        .object()
        .ok()?
        .try_into_commit()
        .ok()?
        .tree()
        .ok()?;
    for path in note_paths(sha) {
        if let Ok(Some(entry)) = tree.clone().lookup_entry_by_path(&path)
            && let Ok(blob) = entry.object()
        {
            return String::from_utf8(blob.data.clone()).ok();
        }
    }
    None
}

/// The ref holding review notes. One name, used by the check and by
/// the message that tells a contributor how to write one.
pub const NOTES_REF: &str = "refs/notes/reviews";

/// Resolve a pushed sha to the commit it names. A tag peels to one.
///
/// Returns the input unchanged where the object is missing or names no
/// commit. A caller then reports "no note" for it, which is correct:
/// an unreadable object carries no review.
fn peel(cwd: &std::path::Path, sha: &str) -> String {
    let peeled = || -> Option<String> {
        let repo = gix::discover(cwd).ok()?;
        let object = repo.rev_parse_single(sha).ok()?.object().ok()?;
        Some(
            object
                .peel_to_kind(gix::object::Kind::Commit)
                .ok()?
                .id
                .to_string(),
        )
    };
    peeled().unwrap_or_else(|| sha.to_string())
}

/// What the check decided about one tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub commit: String,
    pub remote_ref: String,
    pub faults: Vec<Missing>,
}

/// Check every pushed tip against the declared reviews.
///
/// `head_override` replaces the pushed refs when a pull request event
/// supplies one, because the checked-out merge commit is not what a
/// reviewer read.
#[must_use]
pub fn check(cwd: &std::path::Path, refs: &[PushedRef], required: &[String]) -> Vec<Verdict> {
    refs.iter()
        .map(|r| {
            let commit = peel(cwd, &r.local_sha);
            let note = note_body(cwd, &commit);
            Verdict {
                faults: shortfall(note.as_deref(), required, &commit),
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

    const SHA: &str = "4f1c2ab9d0e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9";

    fn ok_line(review: &str) -> String {
        format!("signoff[{review}] PASS {SHA} removed the guard and the named test went red\n")
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
    fn a_missing_note_is_a_fault() {
        assert_eq!(
            shortfall(None, &req(&["fresh-eyes"]), SHA),
            vec![Missing::Note]
        );
    }

    #[test]
    fn a_signed_off_note_passes() {
        let note = ok_line("fresh-eyes") + &ok_line("mutation");
        assert!(shortfall(Some(&note), &req(&["fresh-eyes", "mutation"]), SHA).is_empty());
    }

    #[test]
    fn prose_around_the_lines_is_ignored() {
        let note = format!(
            "A reviewer read the change.\n\n{}\nNotes follow.\n",
            ok_line("fresh-eyes")
        );
        assert!(shortfall(Some(&note), &req(&["fresh-eyes"]), SHA).is_empty());
    }

    #[test]
    fn prose_naming_a_review_does_not_sign_it_off() {
        // The substring design passed this. A note about a review reads
        // exactly like a record of one.
        let note = "mutation: skipped this round, see the ticket\n";
        assert_eq!(
            shortfall(Some(note), &req(&["mutation"]), SHA),
            vec![Missing::Reviews(req(&["mutation"]))]
        );
    }

    #[test]
    fn a_failed_signoff_refuses_the_push() {
        let note = format!("signoff[fresh-eyes] FAIL {SHA} the guard has no test\n");
        assert_eq!(
            shortfall(Some(&note), &req(&["fresh-eyes"]), SHA),
            vec![Missing::Failed(req(&["fresh-eyes"]))]
        );
    }

    #[test]
    fn a_failed_signoff_for_an_undeclared_review_still_refuses() {
        // A reviewer who wrote FAIL meant it, whatever the policy lists.
        let note =
            ok_line("fresh-eyes") + &format!("signoff[security] FAIL {SHA} a hole is open\n");
        assert_eq!(
            shortfall(Some(&note), &req(&["fresh-eyes"]), SHA),
            vec![Missing::Failed(req(&["security"]))]
        );
    }

    #[test]
    fn a_signoff_naming_another_commit_is_refused() {
        // Without this a sign-off copies forward onto a commit nobody
        // read, and nothing says so.
        let other = "0000000000000000000000000000000000000001";
        let note = format!("signoff[fresh-eyes] PASS {other} read it closely enough\n");
        assert_eq!(
            shortfall(Some(&note), &req(&["fresh-eyes"]), SHA),
            vec![Missing::WrongCommit {
                review: "fresh-eyes".to_string(),
                named: other.to_string()
            }]
        );
    }

    #[test]
    fn a_short_sha_matches_the_commit() {
        let note = format!(
            "signoff[fresh-eyes] PASS {} read it closely enough\n",
            &SHA[..7]
        );
        assert!(shortfall(Some(&note), &req(&["fresh-eyes"]), SHA).is_empty());
    }

    #[test]
    fn two_signoffs_for_one_review_are_refused() {
        let note = ok_line("fresh-eyes") + &ok_line("fresh-eyes");
        assert_eq!(
            shortfall(Some(&note), &req(&["fresh-eyes"]), SHA),
            vec![Missing::Duplicate("fresh-eyes".to_string())]
        );
    }

    #[test]
    fn evidence_under_the_floor_is_refused() {
        let note = format!("signoff[fresh-eyes] PASS {SHA} looked\n");
        assert_eq!(
            shortfall(Some(&note), &req(&["fresh-eyes"]), SHA),
            vec![Missing::ThinEvidence {
                review: "fresh-eyes".to_string(),
                words: 1
            }]
        );
    }

    #[test]
    fn a_line_not_anchored_at_the_start_is_not_a_signoff() {
        let note = format!("  signoff[fresh-eyes] PASS {SHA} indented so it does not count\n");
        assert_eq!(
            shortfall(Some(&note), &req(&["fresh-eyes"]), SHA),
            vec![Missing::Reviews(req(&["fresh-eyes"]))]
        );
    }

    #[test]
    fn an_unknown_verdict_is_not_a_signoff() {
        let note = format!("signoff[fresh-eyes] MAYBE {SHA} not a verdict\n");
        assert_eq!(
            shortfall(Some(&note), &req(&["fresh-eyes"]), SHA),
            vec![Missing::Reviews(req(&["fresh-eyes"]))]
        );
    }

    #[test]
    fn an_empty_review_name_is_not_a_signoff() {
        let note = format!("signoff[] PASS {SHA} names no review\n");
        assert!(parse_signoffs(&note).is_empty());
    }

    #[test]
    fn an_empty_policy_requires_no_note() {
        // `reviews: []` states that a repo requires no review. An absent
        // declaration is a different thing and the caller refuses it.
        assert!(shortfall(None, &[], SHA).is_empty());
    }

    #[test]
    fn every_unsigned_review_is_named_at_once() {
        let note = ok_line("fresh-eyes");
        assert_eq!(
            shortfall(
                Some(&note),
                &req(&["fresh-eyes", "mutation", "security"]),
                SHA
            ),
            vec![Missing::Reviews(req(&["mutation", "security"]))]
        );
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
    fn a_nested_head_object_does_not_win_over_the_real_one() {
        // The string search walked to the first "sha" after the first
        // "head", so a nested object took the answer.
        let body = r#"{"pull_request":{"head":{"repo":{"sha":"1111111111111111111111111111111111111111"},"sha":"2222222222222222222222222222222222222222"}}}"#;
        assert_eq!(
            head_sha_from_event(body).as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn an_absent_head_sha_does_not_fall_through_to_base() {
        // The search fell through to base.sha, which names a commit
        // that is already reviewed, so the check passed.
        let body = r#"{"pull_request":{"base":{"sha":"1111111111111111111111111111111111111111"},"head":{}}}"#;
        assert_eq!(head_sha_from_event(body), None);
    }

    #[test]
    fn malformed_json_names_no_head() {
        assert_eq!(head_sha_from_event("not json at all"), None);
    }

    #[test]
    fn a_payload_with_no_head_yields_nothing() {
        assert_eq!(head_sha_from_event(r#"{"pull_request":{}}"#), None);
    }

    #[test]
    fn a_short_sha_in_a_payload_is_refused() {
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

    #[test]
    fn note_paths_covers_the_flat_form_and_every_fanout_depth() {
        let sha = "abcdef0123456789abcdef0123456789abcdef01";
        let paths = note_paths(sha);
        assert_eq!(paths[0], sha, "the flat form comes first");
        assert_eq!(paths[1], format!("ab/{}", &sha[2..]));
        assert_eq!(paths[2], format!("ab/cd/{}", &sha[4..]));
        assert_eq!(paths[3], format!("ab/cd/ef/{}", &sha[6..]));
        assert_eq!(paths.len(), 4);
    }

    #[test]
    fn every_note_path_rejoins_to_the_sha_it_names() {
        // The guarantee against reading another commit's note: a
        // candidate path's components concatenate back to this sha.
        let sha = "abcdef0123456789abcdef0123456789abcdef01";
        for path in note_paths(sha) {
            assert_eq!(path.replace('/', ""), sha, "`{path}` names another commit");
        }
    }

    #[test]
    fn a_sha_that_is_not_hex_gets_no_fanout_and_does_not_panic() {
        // A multi-byte character on stdin panicked the gate, because
        // the fanout sliced by byte index without checking.
        for bad in ["\u{20ac}abc", "not-a-sha", "", "zz"] {
            let paths = note_paths(bad);
            assert_eq!(paths, vec![bad.to_string()], "`{bad}` gained a fanout");
        }
    }

    #[test]
    fn a_short_sha_in_a_signoff_is_refused() {
        // `PASS 5` matched one commit in sixteen, which is the
        // carry-forward the commit binding exists to stop.
        let commit = "5abcdef0123456789abcdef0123456789abcdef0";
        let required = vec!["fresh-eyes".to_string()];
        for named in ["5", "5a", "5abcde"] {
            let note = format!("signoff[fresh-eyes] PASS {named} read every guard closely");
            assert_eq!(
                shortfall(Some(&note), &required, commit),
                vec![Missing::ShortCommit {
                    review: "fresh-eyes".to_string(),
                    named: named.to_string(),
                }],
                "`{named}` was accepted as a sha"
            );
        }
    }

    #[test]
    fn a_seven_character_sha_in_a_signoff_is_accepted() {
        let commit = "5abcdef0123456789abcdef0123456789abcdef0";
        let required = vec!["fresh-eyes".to_string()];
        let note = "signoff[fresh-eyes] PASS 5abcdef read every guard closely";
        assert!(shortfall(Some(note), &required, commit).is_empty());
    }

    #[test]
    fn a_signoff_naming_another_commit_is_still_refused() {
        // The floor must not swallow the wrong-commit case.
        let commit = "5abcdef0123456789abcdef0123456789abcdef0";
        let required = vec!["fresh-eyes".to_string()];
        let note = "signoff[fresh-eyes] PASS 9999999 read every guard closely";
        assert_eq!(
            shortfall(Some(note), &required, commit),
            vec![Missing::WrongCommit {
                review: "fresh-eyes".to_string(),
                named: "9999999".to_string(),
            }]
        );
    }

    #[test]
    fn short_truncates_on_a_character_boundary() {
        // Slicing by byte panicked the gate when a multi-byte
        // character sat across the cut at byte 7.
        assert_eq!(short("abcdef\u{e9}"), "abcdef\u{e9}");
        assert_eq!(short("abcdef\u{e9}ghij"), "abcdef\u{e9}");
        assert_eq!(short("\u{20ac}abc"), "\u{20ac}abc");
        assert_eq!(short("abcdef0123456789"), "abcdef0");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }

    #[test]
    fn a_truncated_stdin_line_is_counted() {
        // One line reading `refs/heads/main` left no refs and
        // non-empty stdin, so the check reported nothing to do and
        // exited 0. A caller that lost the stream passed the gate.
        assert_eq!(truncated_lines("refs/heads/main\n"), 1);
        assert_eq!(truncated_lines("refs/heads/main abc\n"), 1);
        assert_eq!(truncated_lines("a b c\n"), 1);
        assert_eq!(truncated_lines("a b c d\n"), 0);
        assert_eq!(truncated_lines(""), 0);
        assert_eq!(truncated_lines("\n  \n"), 0);
        assert_eq!(truncated_lines("a b c d\nrefs/heads/x\n"), 1);
    }

    #[test]
    fn an_uppercase_abbreviation_names_the_same_commit() {
        // git accepts an uppercase abbreviation, so a reviewer who
        // pasted one named the right commit and was told otherwise.
        let commit = "5abcdef0123456789abcdef0123456789abcdef0";
        let required = vec!["fresh-eyes".to_string()];
        let note = "signoff[fresh-eyes] PASS 5ABCDEF read every guard closely";
        assert!(shortfall(Some(note), &required, commit).is_empty());
    }

    #[test]
    fn a_sha256_length_signoff_is_accepted() {
        let commit = "a".repeat(64);
        let required = vec!["fresh-eyes".to_string()];
        let note = format!("signoff[fresh-eyes] PASS {commit} read every guard closely");
        assert!(shortfall(Some(&note), &required, &commit).is_empty());
    }

    #[test]
    fn a_signoff_sha_over_the_ceiling_is_refused() {
        let commit = "5abcdef0123456789abcdef0123456789abcdef0";
        let required = vec!["fresh-eyes".to_string()];
        let named = "a".repeat(65);
        let note = format!("signoff[fresh-eyes] PASS {named} read every guard closely");
        assert_eq!(
            shortfall(Some(&note), &required, commit),
            vec![Missing::ShortCommit {
                review: "fresh-eyes".to_string(),
                named,
            }]
        );
    }
}
