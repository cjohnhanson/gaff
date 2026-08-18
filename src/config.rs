//! The repo-level config: `.gaff/gaff.yml`. It holds data only.
//!
//! gaff executes nothing that a repo declares. A repo declares the
//! reminder text and cadences. Anything that runs code lives in the
//! user-scoped config. Version 0 has no such config.
//!
//! A malformed config degrades loudly, and it never blocks. The caller
//! prints a warning on stderr, writes a marker, and continues without
//! reminders.

use std::path::Path;

use serde::Deserialize;

pub const CONFIG_PATH: &str = ".gaff/gaff.yml";
const DEFAULT_MAX_INJECT_BYTES: usize = 4096;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// The named overlays. A profile selects which entries are active
    /// and may override their cadences.
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, Profile>,
    /// The profile that applies when nothing else selects one.
    #[serde(default)]
    pub default_profile: Option<String>,
    /// Which profile switches an agent may make on its own session.
    /// An absent value and an empty list mean different things. An
    /// empty `agent_may_set` says the agent may set nothing, and a
    /// repo must not be able to overrule that by looking unset.
    #[serde(default)]
    pub transitions: Option<Transitions>,
    /// The git-hook entries. gaff writes the hook scripts, and they
    /// call back into gaff, so one config covers both domains.
    #[serde(default)]
    pub git: Vec<crate::githook::GitHook>,
    /// The workflows to generate. gaff cannot run a GitHub event, so
    /// this domain is generated and checked, never executed.
    #[serde(default)]
    pub github: Vec<crate::ghworkflow::Workflow>,
    /// The tool calls to refuse. This is the only feature that blocks.
    #[serde(default)]
    pub guards: Vec<crate::guard::Guard>,
    /// The independent reviews a change must pass before it merges.
    /// Each name is a review skill the repo carries.
    ///
    /// `None` and `Some([])` differ, and the difference is the point.
    /// `None` means nobody stated a policy, and a gate must refuse
    /// rather than require nothing. `Some([])` means an author wrote
    /// `reviews: []` and chose that no review is required.
    #[serde(default)]
    pub reviews: Option<Vec<String>>,
}

/// A profile: a named bundle of entries, guards, and a stop rule.
///
/// A profile declares its own sections, reminders, guards, and stop
/// rule. The effective config for a session is the base entries plus
/// the active profile's bundle. Guards always compose, base plus
/// bundle, so no profile can remove a base guard. Sections and reminders
/// compose base plus bundle unless the bundle sets `base: false`, which
/// delivers only the bundle's own. A profile's handlers live in
/// `handlers.yml` under its own `profiles` map, because a command may
/// come only from that one owner-only file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// When false, the base sections and reminders are not delivered
    /// under this profile; only the bundle's own are. Guards always
    /// compose regardless of this flag.
    #[serde(default = "default_true")]
    pub base: bool,
    /// The bundle's own sections.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// The bundle's own reminders.
    #[serde(default)]
    pub reminders: Vec<Reminder>,
    /// The bundle's own guards. User layer only; a repo bundle's guards
    /// are cleared at load, as a repo's top-level guards are.
    #[serde(default)]
    pub guards: Vec<crate::guard::Guard>,
    /// The bundle's stop rule. User layer only.
    #[serde(default)]
    pub stop: Option<Stop>,
    /// Drop these base entries under this profile. Applied only when
    /// `base` is true; with `base: false` there is nothing to drop.
    #[serde(default)]
    pub disable: Vec<String>,
    /// The byte cap under this profile. `None` keeps the base cap.
    #[serde(default)]
    pub max_inject_bytes: Option<usize>,
    /// Removed. Kept deserializable so a config that still carries one
    /// degrades this one profile with a migration line rather than
    /// failing to parse, which would disarm every guard. `check` fails
    /// on either.
    #[serde(default)]
    pub only: Option<Vec<String>>,
    /// Removed; see `only`.
    #[serde(default)]
    pub cadence: std::collections::BTreeMap<String, Every>,
    /// True when the user config declared this profile. Set at load
    /// time, never read from YAML. A repo may not select a profile the
    /// user wrote for their own use.
    #[serde(skip)]
    pub user: bool,
}

const fn default_true() -> bool {
    true
}

impl Profile {
    /// The removed keys this profile still carries, if any. A non-empty
    /// result degrades the profile at load and fails `check`.
    #[must_use]
    pub fn legacy_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.only.is_some() {
            keys.push("only");
        }
        if !self.cadence.is_empty() {
            keys.push("cadence");
        }
        keys
    }
}

/// A profile's stop rule. Sets a hold at stop time under the id
/// `profile-<name>`. `times` bounds the refusals; `None` holds until
/// the model clears it or the safety valve lets it through.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    /// The text the model reads when the stop is refused.
    pub hold: String,
    /// How many stops to refuse before letting one through.
    #[serde(default)]
    pub times: Option<u32>,
}

/// The transition policy for profile switches.
///
/// Profiles are advisory: gaff blocks nothing, and an agent that can
/// write files can edit `.gaff/gaff.yml` anyway. The policy states
/// intent and refuses the agent-facing path, so a switch an operator
/// did not sanction is at least not a supported one.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transitions {
    /// The profiles an agent may select for itself. A profile absent
    /// from this list is human-only.
    #[serde(default)]
    pub agent_may_set: Vec<String>,
}

impl Transitions {
    /// Whether an agent may select `name`.
    #[must_use]
    pub fn agent_may_set(&self, name: &str) -> bool {
        self.agent_may_set.iter().any(|p| p == name)
    }
}

impl Config {
    /// Apply a profile overlay and return the effective config.
    ///
    /// An unknown name applies nothing and warns. A typo must never
    /// silently empty the config, because a silent empty config looks
    /// exactly like a working one.
    ///
    /// The result is the effective config: base entries plus the active
    /// bundle. A bundle entry comes before the base entries of its kind,
    /// so the reason the profile exists spends the byte cap and the
    /// session-start budget first. Guards always compose. Sections and
    /// reminders drop the base ones when `base` is false, and drop any
    /// named in `disable` when `base` is true. A bundle entry that
    /// shadows a base entry of the same kind by name replaces it, which
    /// is how a bundle retimes a base reminder.
    #[must_use]
    pub fn with_profile(&self, name: Option<&str>) -> Self {
        let mut out = self.clone();
        let Some(name) = name else {
            return out;
        };
        let Some(profile) = self.profiles.get(name) else {
            eprintln!("gaff: unknown profile `{name}`. Using the base config.");
            return out;
        };

        let shadowed_r: Vec<String> = profile.reminders.iter().map(|r| r.name.clone()).collect();
        let shadowed_s: Vec<String> = profile.sections.iter().map(|s| s.name.clone()).collect();

        let keep_base = |n: &str| profile.base && !profile.disable.iter().any(|d| d == n);
        out.reminders
            .retain(|r| keep_base(&r.name) && !shadowed_r.contains(&r.name));
        out.sections
            .retain(|s| keep_base(&s.name) && !shadowed_s.contains(&s.name));

        // Bundle entries first, then the surviving base entries.
        let mut reminders = profile.reminders.clone();
        reminders.append(&mut out.reminders);
        out.reminders = reminders;
        let mut sections = profile.sections.clone();
        sections.append(&mut out.sections);
        out.sections = sections;

        out.guards.extend(profile.guards.iter().cloned());

        if let Some(cap) = profile.max_inject_bytes {
            out.max_inject_bytes = cap;
        }
        out
    }

    /// The active profile's stop rule, if it declares one.
    #[must_use]
    pub fn profile_stop(&self, name: Option<&str>) -> Option<&Stop> {
        self.profiles.get(name?)?.stop.as_ref()
    }
}

/// Merge the reviews of the two layers.
///
/// The repo states the policy, and the user adds to it. A repo that
/// states nothing yields nothing, whatever the user declares: gate
/// policy belongs to the repo, and a truncated repo config beside a
/// user config would otherwise read as a policy the repo never wrote.
///
/// Where the repo states one, the union holds and neither layer drops
/// the other's name. A gate reads this list, so a layer that replaced
/// it could require nothing and pass a change that nobody reviewed.
fn merge_reviews(user: Option<Vec<String>>, repo: Option<Vec<String>>) -> Option<Vec<String>> {
    let repo = repo?;
    let mut names = user.unwrap_or_default();
    for name in repo {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Some(names)
}

/// Strip a repo profile of everything that would reach a user entry or
/// carry a blocking rule.
///
/// A repo bundle governs the repo's own entries and nothing the user
/// declared. It may not silence a base user entry (so `base` is forced
/// true and `disable` drops only its own names), it may not carry a
/// guard or a stop rule (those are the only things that block, and they
/// are user-only, as a repo's top-level guards are), and its cap may
/// not fall below the user's.
fn sanitize_repo_profile(
    mut profile: Profile,
    user_entries: &[String],
    user_cap: usize,
) -> Profile {
    profile.base = true;
    profile.disable.retain(|d| !user_entries.contains(d));
    if !profile.guards.is_empty() {
        eprintln!("gaff: a repo profile declares guards, which are user-only. Dropping them.");
        profile.guards.clear();
    }
    if profile.stop.is_some() {
        eprintln!("gaff: a repo profile declares a stop rule, which is user-only. Dropping it.");
        profile.stop = None;
    }
    if let Some(cap) = profile.max_inject_bytes
        && !user_entries.is_empty()
    {
        profile.max_inject_bytes = Some(cap.max(user_cap));
    }
    profile.user = false;
    profile
}

/// Drop a repo entry that took a user entry's name across kinds.
///
/// Reminders and sections share one pending-marker namespace, keyed by
/// name. A repo section under a user reminder's name consumes that
/// reminder's marker, so the reminder stops firing.
fn drop_cross_kind_clashes(cfg: &mut Config, reminders: &[String], sections: &[String]) {
    cfg.sections.retain(|s| {
        let clash = reminders.contains(&s.name) && !sections.contains(&s.name);
        if clash {
            eprintln!(
                "gaff: the repo declares a section named `{}`, which is a user reminder. Keeping the reminder.",
                s.name
            );
        }
        !clash
    });
    cfg.reminders.retain(|r| {
        let clash = sections.contains(&r.name) && !reminders.contains(&r.name);
        if clash {
            eprintln!(
                "gaff: the repo declares a reminder named `{}`, which is a user section. Keeping the section.",
                r.name
            );
        }
        !clash
    });
}

/// Resolve the active profile name. The resolution path is the flag,
/// then the environment, then the session state, then `.gaff/profile`,
/// then the config default. The first hit wins.
#[must_use]
pub fn resolve_profile(
    flag: Option<&str>,
    env: Option<&str>,
    session: Option<&str>,
    gaff_dir: &Path,
    config: &Config,
) -> Option<String> {
    let from_file = || {
        let name = std::fs::read_to_string(gaff_dir.join("profile"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        // `.gaff/profile` is a repo file, and a clone carries it. It
        // selects a repo profile only, for the same reason a repo
        // `default_profile` does.
        if config.profiles.get(&name).is_some_and(|p| p.user) {
            eprintln!(
                "gaff: .gaff/profile names `{name}`, which the user declared. A repo may not select a user profile, so it is ignored."
            );
            return None;
        }
        Some(name)
    };
    // `GAFF_PROFILE` is not a trusted channel. An agent reading
    // repo-supplied text can export it in one Bash call, and a repo
    // reaches it through direnv, a Makefile, or a devcontainer. It
    // therefore answers to the same policy as the agent-facing switch:
    // where the user stated a transition policy, a profile absent from
    // `agent_may_set` is human-only and the variable cannot select it.
    let from_env = || {
        let name = env.map(ToString::to_string)?;
        let allowed = config
            .transitions
            .as_ref()
            .is_some_and(|t| t.agent_may_set(&name));
        // A bundle that carries a guard or a stop rule is law, not just
        // context. The environment must not select it unless the user
        // named it in `agent_may_set`, whether or not a transition policy
        // is declared at all. A repo reaches `GAFF_PROFILE` through
        // direnv, and an agent exports it in one call.
        let carries_law = config
            .profiles
            .get(&name)
            .is_some_and(|p| p.user && (!p.guards.is_empty() || p.stop.is_some()));
        if carries_law && !allowed {
            eprintln!(
                "gaff: GAFF_PROFILE names `{name}`, which carries guards or a stop rule and is human-only. Add it to transitions.agent_may_set to allow it, or use `gaff profile set` from a terminal."
            );
            return None;
        }
        if let Some(t) = &config.transitions
            && config.profiles.contains_key(&name)
            && !t.agent_may_set(&name)
        {
            eprintln!(
                "gaff: GAFF_PROFILE names `{name}`, which is human-only. Add it to transitions.agent_may_set to allow it, or use `gaff profile set` from a terminal."
            );
            return None;
        }
        Some(name)
    };
    flag.map(ToString::to_string)
        .or_else(from_env)
        .or_else(|| session.map(ToString::to_string))
        .or_else(from_file)
        .or_else(|| config.default_profile.clone())
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
            profiles: std::collections::BTreeMap::new(),
            default_profile: None,
            transitions: None,
            git: Vec::new(),
            github: Vec::new(),
            guards: Vec::new(),
            reviews: None,
        }
    }
}

const fn default_max_inject_bytes() -> usize {
    DEFAULT_MAX_INJECT_BYTES
}

/// Resolve a section file path under `gaff_dir` and confine it there.
///
/// A section file is repo data. It must not read outside `.gaff/`. This
/// rejects an absolute path and any `..` that escapes the directory,
/// which stops a committed config from reading a file into the model's
/// context. The check is lexical, so it needs no file on disk and works
/// the same in `gaff check` and at read time.
///
/// # Errors
/// Returns the offending path string when the file is absolute or leaves
/// `gaff_dir`.
pub fn confine_section_path(gaff_dir: &Path, file: &str) -> Result<std::path::PathBuf, String> {
    let candidate = Path::new(file);
    if candidate.is_absolute() {
        return Err(file.to_string());
    }
    let mut depth: i32 = 0;
    for component in candidate.components() {
        match component {
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(file.to_string());
                }
            }
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(file.to_string());
            }
        }
    }
    Ok(gaff_dir.join(candidate))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reminder {
    pub name: String,
    pub every: Every,
    pub text: String,
    /// True when the user config declared this reminder. Set at load
    /// time, never read from YAML.
    #[serde(skip)]
    pub user: bool,
}

/// A prime section: a markdown file under `.gaff/`. gaff injects the
/// whole file at `SessionStart`. gaff injects it again on its refresh
/// cadence.
///
/// Sections and reminders share one namespace, because the pending state
/// and the cursor state use the name as the key. A name must be unique
/// across both.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub name: String,
    /// The path to the section body, relative to the directory of the
    /// config that declared the section: `.gaff/` for a repo section,
    /// and `$HOME/.config/gaff/` for a user section.
    pub file: String,
    #[serde(default)]
    pub refresh: Every,
    /// True when the user config declared this section. Set at load
    /// time, never read from YAML.
    #[serde(skip)]
    pub user: bool,
}

/// Resolve a section body against the directory of the config that
/// declared it.
///
/// A user section names a file next to the user's own config. Resolving
/// every section against the repo's `.gaff/` left the user's file
/// unread, and let any cloned repo supply the body under the user's
/// section name. That put repo-authored text into the model's session
/// framing labelled as the user's own.
///
/// # Errors
/// Returns a reader-facing message when the path escapes its root, or
/// when a user section has no user config directory to resolve against.
pub fn read_section_body(section: &Section, gaff_dir: &Path) -> Result<String, String> {
    let path = section_path(section, gaff_dir)?;
    // The lexical confinement above is not enough on its own. It reads
    // the path string, and a symlink is resolved by the filesystem
    // afterwards. git carries a symlink through a clone, so a committed
    // `.gaff/notes.md -> ~/.ssh/id_rsa` passed every lexical check and
    // put the key into the model's context, and a link to `/dev/zero`
    // wedged every hook event in the session while it ate memory.
    //
    // A repo section therefore reads a regular file and nothing else. A
    // user section may be a link, for the same reason the user config
    // may: a dotfile manager installs it that way, and anyone who can
    // write inside `$HOME/.config/gaff` already owns the machine.
    // A repo section must resolve to a file that is really inside the
    // repo's `.gaff/`. Testing the last component alone was not enough:
    // any directory along the way could be a link, and the filesystem
    // resolves it before the test runs. A committed `.gaff/sub -> /`,
    // or `.gaff` itself as a link, moved the whole root out of the repo
    // while the path string stayed clean. The comparison has to be
    // positional, so both sides are canonicalized.
    if !section.user {
        // `.gaff` itself must be a real directory in the repo. If it is
        // a link, canonicalizing it would follow the link and then
        // every file under the target would compare as "inside", which
        // is the same hole one level up.
        if std::fs::symlink_metadata(gaff_dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "section `{}`: {} is a symlink. gaff reads a repo section from a real directory in the repo.",
                section.name,
                gaff_dir.display()
            ));
        }
        let root = std::fs::canonicalize(gaff_dir).map_err(|e| {
            format!(
                "section `{}`: cannot resolve {}: {e}",
                section.name,
                gaff_dir.display()
            )
        })?;
        let real = std::fs::canonicalize(&path).map_err(|e| {
            format!(
                "section `{}`: cannot resolve {}: {e}",
                section.name,
                path.display()
            )
        })?;
        if !real.starts_with(&root) {
            return Err(format!(
                "section `{}`: {} resolves to {}, outside {}. A repo section reads a file inside .gaff/ only, because a link can point at any file gaff can read.",
                section.name,
                path.display(),
                real.display(),
                root.display()
            ));
        }
        // Read the confined path rather than the name that was
        // checked. They are the same file unless something swapped the
        // name in between, and then this reads the one that passed.
        return read_confined(section, &real, true);
    }
    read_confined(section, &path, false)
}

/// Open a section body and answer every check from the one descriptor.
///
/// `no_follow` refuses a final component that is a symlink, which is
/// the last thing a concurrent writer could swap after the path was
/// canonicalized. It is set for a repo section only: a user section may
/// be a link, because a dotfile manager installs it that way.
fn read_confined(section: &Section, path: &Path, no_follow: bool) -> Result<String, String> {
    // Open once, and answer every question from that one descriptor.
    //
    // Resolving the path a second time for `metadata` and a third for
    // the read left two windows in which the name could be swapped for
    // a link pointing out of the repo. The checks then described a
    // different file from the one that was read.
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // O_NONBLOCK makes the open of a FIFO return rather than wait
        // for a writer. Putting the type check behind the open meant a
        // stray FIFO wedged every hook event in the session; the flag
        // is a no-op on a regular file, and `fstat` below still refuses
        // the FIFO.
        let mut flags = libc::O_NONBLOCK;
        if no_follow {
            flags |= libc::O_NOFOLLOW;
        }
        open.custom_flags(flags);
    }
    #[cfg(not(unix))]
    let _ = no_follow;
    let mut file = open.open(path).map_err(|e| {
        format!(
            "section `{}`: cannot read {}: {e}",
            section.name,
            path.display()
        )
    })?;
    let meta = file.metadata().map_err(|e| {
        format!(
            "section `{}`: cannot read {}: {e}",
            section.name,
            path.display()
        )
    })?;
    if !meta.is_file() {
        return Err(format!(
            "section `{}`: {} is not a regular file",
            section.name,
            path.display()
        ));
    }
    if meta.len() > MAX_SECTION_BYTES {
        return Err(format!(
            "section `{}`: {} is larger than {MAX_SECTION_BYTES} bytes",
            section.name,
            path.display()
        ));
    }
    let mut body = String::new();
    std::io::Read::read_to_string(&mut file, &mut body).map_err(|e| {
        format!(
            "section `{}`: cannot read {}: {e}",
            section.name,
            path.display()
        )
    })?;
    Ok(body)
}

/// The largest section body gaff will read. A section is injected whole
/// and the cap on a flush is far below this, so this bound exists to
/// stop an unbounded read, not to size the injection.
const MAX_SECTION_BYTES: u64 = 1024 * 1024;

/// Resolve a section body path against the directory of the config that
/// declared it.
pub fn section_path(section: &Section, gaff_dir: &Path) -> Result<std::path::PathBuf, String> {
    let (root, label) = if section.user {
        let Some(dir) = crate::handler::config_dir() else {
            return Err(format!(
                "section `{}`: no user config directory. Set HOME.",
                section.name
            ));
        };
        (dir, "the user config directory")
    } else {
        (gaff_dir.to_path_buf(), ".gaff/")
    };
    confine_section_path(&root, &section.file)
        .map_err(|bad| format!("section `{}`: the path {bad} leaves {label}", section.name))
}

/// A cadence: fire every N counted events of the given unit.
#[derive(Debug, Default, Clone, Deserialize)]
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
    /// The repo config failed to parse, and the user config stands
    /// alone. The guards survive, and the state is still degraded, so
    /// `gaff doctor` must say so.
    Degraded(Config),
    Broken(String),
}

/// The user-scoped data config. It holds the same keys as a repo
/// config, and it never holds handlers.
///
/// Handlers stay in their own file, so the security boundary stays
/// trivially checkable: a command can only come from `handlers.yml`.
#[must_use]
pub fn user_config_path() -> Option<std::path::PathBuf> {
    crate::handler::config_dir().map(|d| d.join("gaff.yml"))
}

/// Load and parse the user config alone.
///
/// `Ok(None)` means there is none. This is the layer that holds guards,
/// and `gaff check --handlers` validates it directly.
///
/// # Errors
/// Returns a reader-facing message when the file cannot be read or does
/// not parse.
pub fn load_user(path: &Path) -> Result<Option<Config>, String> {
    let Some(text) = read_user_config_file(path)? else {
        return Ok(None);
    };
    match serde_yaml_ng::from_str::<Config>(&text) {
        Ok(cfg) => Ok(Some(cfg)),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Load the user config, then lay the repo config over it.
///
/// A person works in many repos and wants some reminders everywhere.
/// The repo is the more specific scope, so a repo entry wins over a
/// user entry of the same name.
#[must_use]
pub fn load_layered(cwd: &Path) -> Loaded {
    let user = match user_config_path() {
        None => {
            // No HOME means no user config, and guards live only
            // there. Every guard is off, and the quietest failure is
            // the worst one for a rule that blocks.
            eprintln!(
                "gaff: no HOME, so the user config could not be read. Any guard declared there is not active."
            );
            None
        }
        // HOME is set but is not a directory. That is an environment
        // fault rather than "the user declared nothing", and it has the
        // same effect on a guard, so it gets the same warning.
        Some(p) if !std::env::var("HOME").is_ok_and(|h| Path::new(&h).is_dir()) => {
            eprintln!(
                "gaff: HOME does not name a directory, so {} could not be read. Any guard declared there is not active.",
                p.display()
            );
            None
        }
        Some(p) => match read_user_config_file(&p) {
            Ok(Some(text)) => match serde_yaml_ng::from_str::<Config>(&text) {
                Ok(mut cfg) => {
                    // Stamp the layer while the two configs are still
                    // separate. After the merge there is no way to tell
                    // which layer an entry came from, and both the
                    // section root and the profile-selection rule need
                    // to know.
                    for s in &mut cfg.sections {
                        s.user = true;
                    }
                    for r in &mut cfg.reminders {
                        r.user = true;
                    }
                    for p in cfg.profiles.values_mut() {
                        p.user = true;
                        // A bundle section names a file next to the user
                        // config; stamp its entries so `section_path`
                        // resolves against the user directory and not the
                        // repo's `.gaff/`, the same bug fixed for the top
                        // level one level up.
                        for s in &mut p.sections {
                            s.user = true;
                        }
                        for r in &mut p.reminders {
                            r.user = true;
                        }
                    }
                    for g in &mut cfg.git {
                        g.user = true;
                    }
                    for w in &mut cfg.github {
                        w.user = true;
                    }
                    // A user profile that still carries a removed key is
                    // dropped with a migration line, so a stale key never
                    // reaches the merge and never disarms a guard.
                    drop_legacy_profiles(&mut cfg);
                    Some(cfg)
                }
                Err(e) => return Loaded::Broken(format!("{}: {e}", p.display())),
            },
            Ok(None) => None,
            Err(e) => return Loaded::Broken(e),
        },
    };
    match (user, load(cwd)) {
        (None, repo) => repo,
        // No repo config states no review policy, and the user's list
        // must not stand in for one. Every other user field survives:
        // a repo that is absent cannot revoke what the user declared.
        (Some(mut user), Loaded::Absent) => {
            user.reviews = None;
            Loaded::Ok(user)
        }
        (Some(user), Loaded::Ok(repo) | Loaded::Degraded(repo)) => {
            Loaded::Ok(user.overlaid_with(repo))
        }
        // A repo that cannot be parsed contributes nothing. It must
        // not delete what the user declared, because the user's guards
        // are refusals and a repo could otherwise switch them all off
        // with one bad line. Its review policy is unreadable, though,
        // so no list is stated.
        (Some(mut user), Loaded::Broken(err)) => {
            eprintln!("gaff: {CONFIG_PATH} is not valid: {err}. Using the user config alone.");
            user.reviews = None;
            Loaded::Degraded(user)
        }
    }
}

impl Config {
    /// Lay `repo` over `self`, where `self` is the user config.
    ///
    /// Reminders, sections, and profiles merge by name, and the repo
    /// entry replaces the user entry it shadows. A scalar takes the
    /// repo value when the repo sets one.
    ///
    /// `transitions` is the exception: the user value wins whenever the
    /// user sets one. That field says which profiles an agent may grant
    /// itself, and a repo must not widen it.
    #[must_use]
    pub fn overlaid_with(mut self, repo: Self) -> Self {
        let user_set_transitions = self.transitions.is_some();
        // A repo may not redefine a profile the user declared. The user
        // sanctions a profile by name, and a repo that could rewrite
        // that name would decide what the sanctioned profile does.
        let user_named: Vec<String> = self.profiles.keys().cloned().collect();
        // Capture these before any merge. Reminders and sections share
        // one pending-marker namespace, so a repo section taking a user
        // reminder's name consumes that reminder's marker.
        let user_reminder_names: Vec<String> =
            self.reminders.iter().map(|r| r.name.clone()).collect();
        let user_section_names: Vec<String> =
            self.sections.iter().map(|s| s.name.clone()).collect();

        // A repo may add a reminder or a section, and it may not take
        // the name of one the user declared. Taking the name replaces
        // the user's text with the repo's under the user's label, and
        // the model is given no way to tell the two apart. Every other
        // kind resolves a name collision in the user's favour; these
        // two were the last exceptions.
        for r in repo.reminders {
            if user_reminder_names.contains(&r.name) {
                eprintln!(
                    "gaff: the repo declares a reminder named `{}`, which the user declared. Keeping the user's.",
                    r.name
                );
                continue;
            }
            self.reminders.push(r);
        }
        for mut s in repo.sections {
            if user_section_names.contains(&s.name) {
                eprintln!(
                    "gaff: the repo declares a section named `{}`, which the user declared. Keeping the user's.",
                    s.name
                );
                continue;
            }
            s.user = false;
            self.sections.push(s);
        }
        // The names the *user* declared. These were captured before the
        // repo's entries were merged in. Computing them afterwards
        // swept up the repo's own names, so a repo profile could not
        // filter the repo's own entries either and every repo profile
        // became a silent no-op.
        let user_entries: Vec<String> = user_reminder_names
            .iter()
            .chain(user_section_names.iter())
            .cloned()
            .collect();
        let cap = self.max_inject_bytes;
        for (name, profile) in repo.profiles {
            let profile = sanitize_repo_profile(profile, &user_entries, cap);
            if user_named.contains(&name) {
                eprintln!(
                    "gaff: the repo redefines the profile `{name}`, which the user declared. Keeping the user's."
                );
                continue;
            }
            self.profiles.insert(name, profile);
        }
        self.reviews = merge_reviews(self.reviews.take(), repo.reviews);

        // A repo may add a git entry or a workflow, and it may not
        // replace one the user declared. Both run commands, and the
        // consent a user gave was to their own entry, not to whatever
        // a later pull puts under the same name.
        // A git entry is a blocking check, and both layers installed
        // theirs on purpose. Dropping either one silences a check
        // somebody asked for: dropping the repo's passed a commit the
        // repo meant to gate, and refusing to run anything blocked
        // every commit in the clone. So both run, and the name is
        // reported rather than resolved. Entries are matched by layer
        // where identity matters, which is `use_git`.
        for entry in repo.git {
            if self.git.iter().any(|u| u.name == entry.name) {
                eprintln!(
                    "gaff: the repo and the user both declare a git entry named `{}`. Both run. Rename one of them to tell them apart in the output.",
                    entry.name
                );
            }
            self.git.push(entry);
        }
        for wf in repo.github {
            if self.github.iter().any(|u| u.name == wf.name) {
                eprintln!(
                    "gaff: the repo declares a workflow named `{}`, which the user declared. Keeping the user's.",
                    wf.name
                );
                continue;
            }
            self.github.push(wf);
        }
        drop_cross_kind_clashes(&mut self, &user_reminder_names, &user_section_names);
        // A guard comes from the user config only. A repo cannot add
        // one, and a repo cannot remove one.
        //
        // Guards are the single blocking feature, and a repo is
        // untrusted content. A repo-declared guard is a cloned
        // repository deciding which tool calls its reader may make, and
        // a repo declaring `tool: '.*'` would refuse every call. The
        // repo's guards are dropped here rather than merged.
        // The repo's guards were already dropped by `load`. Nothing to
        // merge here, and nothing to warn about twice.

        // A repo may set the cap, but not so low that it silences what
        // the user declared. A cap of one byte drops every user entry
        // without touching a profile, which is the same silencing a
        // repo profile is refused for.
        if repo.max_inject_bytes != DEFAULT_MAX_INJECT_BYTES {
            let user_declared = !user_entries.is_empty();
            self.max_inject_bytes = if user_declared {
                repo.max_inject_bytes.max(self.max_inject_bytes)
            } else {
                repo.max_inject_bytes
            };
        }
        // A repo may set its own default profile, and it may not point
        // at one the user wrote. A user profile such as `quiet` is a
        // switch the user pulls when they want it; a repo that could
        // name it as the default would decide when the user's own kill
        // switch fires.
        if let Some(name) = repo.default_profile {
            if self.profiles.get(&name).is_some_and(|p| p.user) {
                eprintln!(
                    "gaff: the repo names `{name}` as its default profile, which the user declared. A repo may not select a user profile, so it is ignored."
                );
            } else {
                self.default_profile = Some(name);
            }
        }
        if !user_set_transitions {
            // A repo may state a policy where the user stated none, and
            // it may not name a user profile in it. `agent_may_set` is
            // the one remaining door onto a user profile: a repo that
            // could name `quiet` there would let an agent it prompts
            // fire the user's own kill switch.
            self.transitions = repo.transitions.map(|mut t| {
                t.agent_may_set.retain(|name| {
                    let user_owned = self.profiles.get(name).is_some_and(|p| p.user);
                    if user_owned {
                        eprintln!(
                            "gaff: the repo names `{name}` as agent-settable, and the user declared it. A repo may not open a user profile to an agent, so it is ignored."
                        );
                    }
                    !user_owned
                });
                t
            });
        }
        self
    }
}

/// The largest config gaff will read.
///
/// A repo controls this file, and an unbounded read of a path a repo
/// chose is a way to spend all of a machine's memory.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Read a config file that untrusted content may control.
///
/// The path must be a regular file. A repo can commit a symlink, and
/// git carries it through a clone, so a `.gaff/gaff.yml` pointing at
/// `/dev/zero` would read until the process died, and one pointing at
/// a FIFO would block every tool call forever. Neither is a refusal,
/// and both are worse than one.
fn read_config_file(path: &Path) -> Result<Option<String>, String> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{} is a symlink. gaff reads a regular file only, because a symlink can point at a device or a pipe.",
            path.display()
        ));
    }
    check_and_read(path, &meta)
}

/// Read the user's own config.
///
/// This one follows a symlink, and checks the target. Every mainstream
/// dotfile manager (home-manager, stow, chezmoi) installs
/// `$HOME/.config/gaff/gaff.yml` as a link into a managed store, so
/// refusing a link here disarms the guards for a layout the user did
/// nothing wrong to have. The threat the refusal answers is a *cloned
/// repo* aiming gaff at a device or a pipe; anyone who can write inside
/// `$HOME/.config/gaff` already owns the machine. The regular-file and
/// size checks still apply, to the target.
fn read_user_config_file(path: &Path) -> Result<Option<String>, String> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(None);
    };
    check_and_read(path, &meta)
}

fn check_and_read(path: &Path, meta: &std::fs::Metadata) -> Result<Option<String>, String> {
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} is larger than {MAX_CONFIG_BYTES} bytes",
            path.display()
        ));
    }
    match std::fs::read_to_string(path) {
        // An empty file declares nothing, and it is the shape a
        // truncated write or a bad merge leaves behind. Treating it as
        // absent puts it with the other accidents, so a git hook
        // refuses rather than passing a check that never ran.
        Ok(text) if text.trim().is_empty() => Ok(None),
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Drop every profile that still carries a removed key, with a migration
/// line. Returns whether any was dropped, so the caller can mark the
/// config degraded. Keeping the profile would honor a key the merge no
/// longer reads, so the safe reading is to drop the profile and say why.
fn drop_legacy_profiles(cfg: &mut Config) -> bool {
    let stale: Vec<(String, Vec<&'static str>)> = cfg
        .profiles
        .iter()
        .filter_map(|(name, p)| {
            let keys = p.legacy_keys();
            (!keys.is_empty()).then(|| (name.clone(), keys))
        })
        .collect();
    for (name, keys) in &stale {
        eprintln!(
            "gaff: profile `{name}` uses the removed key(s) {}. Declare the entries inside the profile instead. Ignoring this profile.",
            keys.join(", ")
        );
        cfg.profiles.remove(name);
    }
    !stale.is_empty()
}

/// Problems `check` reports for a removed profile key.
///
/// The raw user and repo config files are parsed, because `load` drops a
/// profile carrying a removed key before `check` sees it. A parse
/// failure is reported by the normal load path, so it is ignored here.
#[must_use]
pub fn legacy_key_problems(cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let files = [user_config_path(), Some(cwd.join(CONFIG_PATH))];
    for path in files.into_iter().flatten() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(cfg) = serde_yaml_ng::from_str::<Config>(&text) else {
            continue;
        };
        for (name, profile) in &cfg.profiles {
            let keys = profile.legacy_keys();
            if !keys.is_empty() {
                out.push(format!(
                    "profile `{name}` uses the removed key(s) {}. Declare the entries inside the profile instead.",
                    keys.join(", ")
                ));
            }
        }
    }
    out
}

#[must_use]
pub fn load(cwd: &Path) -> Loaded {
    let path = cwd.join(CONFIG_PATH);
    let bytes = match read_config_file(&path) {
        Ok(Some(text)) => text,
        Ok(None) => return Loaded::Absent,
        Err(e) => return Loaded::Broken(e),
    };
    match serde_yaml_ng::from_str::<Config>(&bytes) {
        Ok(mut cfg) => {
            // A repo may never declare a guard. Guards are the one
            // blocking feature, and a repo is untrusted content. This
            // is done here rather than at merge time, so a repo cannot
            // reach the guards through a path that skips the merge,
            // such as a machine with no user config at all.
            if !cfg.guards.is_empty() {
                eprintln!(
                    "gaff: {CONFIG_PATH} declares {} guard(s). A repo may not declare a guard, so they are ignored. Guards belong in $HOME/.config/gaff/gaff.yml.",
                    cfg.guards.len()
                );
                cfg.guards.clear();
            }
            // A repo profile may not carry a guard or a stop rule, for
            // the same reason a repo top level may not: both block, and a
            // repo is untrusted content. Cleared here so the no-user-config
            // path (`load_layered` returning this config unmerged) is
            // covered too, not only the merge path in `sanitize_repo_profile`.
            for (name, profile) in &mut cfg.profiles {
                if !profile.guards.is_empty() {
                    eprintln!(
                        "gaff: {CONFIG_PATH} profile `{name}` declares guards. A repo may not, so they are ignored."
                    );
                    profile.guards.clear();
                }
                if profile.stop.is_some() {
                    eprintln!(
                        "gaff: {CONFIG_PATH} profile `{name}` declares a stop rule. A repo may not, so it is ignored."
                    );
                    profile.stop = None;
                }
                profile.base = true;
            }
            if drop_legacy_profiles(&mut cfg) {
                return Loaded::Degraded(cfg);
            }
            Loaded::Ok(cfg)
        }
        // Name this one, because the generic serde message does not say
        // where handlers belong, and the reader has to know that a repo
        // may never declare a command.
        Err(e) if e.to_string().contains("unknown field `handlers`") => Loaded::Broken(format!(
            "{}: a repo may not declare handlers. They are user-scoped, in \
             $HOME/.config/gaff/handlers.yml, because a repo-declared command \
             would run on clone.",
            path.display()
        )),
        // Name the file. A message that did not say which config failed
        // was reported under the repo path even when the user config
        // was the one at fault.
        Err(e) => Loaded::Broken(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reminders_and_cap() {
        let cfg: Config = serde_yaml_ng::from_str(
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
        let cfg: Config = serde_yaml_ng::from_str("reminders: []\n").unwrap();
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

#[cfg(test)]
mod profile_tests {
    use super::*;

    fn base() -> Config {
        serde_yaml_ng::from_str(
            "reminders:\n  - name: a\n    every: {tool_calls: 5}\n    text: A\n  - name: b\n    every: {tool_calls: 7}\n    text: B\nsections:\n  - name: s\n    file: s.md\n    refresh: {prompts: 3}\nprofiles:\n  focus:\n    base: false\n    reminders:\n      - name: a\n        every: {tool_calls: 2}\n        text: A2\n  quiet:\n    disable: [a, b, s]\n    max_inject_bytes: 100\ndefault_profile: focus\ntransitions:\n  agent_may_set: [focus]\n",
        )
        .expect("the fixture config must parse")
    }

    #[test]
    fn a_solo_bundle_delivers_only_its_own_and_shadows_by_name() {
        let cfg = base().with_profile(Some("focus"));
        assert_eq!(cfg.reminders.len(), 1, "base: false drops the base entries");
        assert_eq!(cfg.reminders[0].name, "a");
        assert_eq!(
            cfg.reminders[0].every.tool_calls,
            Some(2),
            "the bundle's own `a` replaces the base `a`, which retimes it"
        );
        assert_eq!(cfg.reminders[0].text, "A2");
        assert!(
            cfg.sections.is_empty(),
            "base: false drops the base section"
        );
    }

    #[test]
    fn a_bundle_composes_over_the_base_when_base_is_true() {
        let cfg: Config = serde_yaml_ng::from_str(
            "reminders:\n  - name: a\n    every: {tool_calls: 5}\n    text: A\nprofiles:\n  add:\n    reminders:\n      - name: c\n        every: {tool_calls: 3}\n        text: C\n",
        )
        .unwrap();
        let out = cfg.with_profile(Some("add"));
        let names: Vec<&str> = out.reminders.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["c", "a"], "the bundle entry comes before the base");
    }

    #[test]
    fn guards_always_compose_base_plus_bundle() {
        let cfg: Config = serde_yaml_ng::from_str(
            "guards:\n  - name: base-g\n    tool: Bash\n    message: no\nprofiles:\n  strict:\n    base: false\n    guards:\n      - name: bundle-g\n        tool: Read\n        message: no\n",
        )
        .unwrap();
        let out = cfg.with_profile(Some("strict"));
        let names: Vec<&str> = out.guards.iter().map(|g| g.name.as_str()).collect();
        assert!(names.contains(&"base-g"), "base: false never drops a guard");
        assert!(names.contains(&"bundle-g"));
    }

    #[test]
    fn a_repo_bundle_loses_guards_and_stop_and_cannot_silence_the_base() {
        let repo: Config = serde_yaml_ng::from_str(
            "profiles:\n  ci:\n    base: false\n    disable: [user-r]\n    guards:\n      - name: repo-g\n        tool: Bash\n        message: no\n    stop:\n      hold: block\n",
        )
        .unwrap();
        let ci = repo.profiles.get("ci").unwrap().clone();
        let sanitized = sanitize_repo_profile(ci, &["user-r".to_string()], 4096);
        assert!(
            sanitized.base,
            "a repo bundle may not run solo over user entries"
        );
        assert!(
            sanitized.guards.is_empty(),
            "a repo bundle carries no guards"
        );
        assert!(
            sanitized.stop.is_none(),
            "a repo bundle carries no stop rule"
        );
        assert!(
            !sanitized.disable.contains(&"user-r".to_string()),
            "a repo bundle may not disable a user entry"
        );
    }

    #[test]
    fn a_profile_with_a_removed_key_is_dropped_at_load() {
        let mut cfg: Config = serde_yaml_ng::from_str(
            "profiles:\n  legacy:\n    only: [a]\n  good:\n    base: false\n",
        )
        .unwrap();
        assert!(
            drop_legacy_profiles(&mut cfg),
            "the legacy profile is dropped"
        );
        assert!(!cfg.profiles.contains_key("legacy"));
        assert!(
            cfg.profiles.contains_key("good"),
            "the good profile survives"
        );
    }

    #[test]
    fn a_dotted_profile_hold_is_found_by_the_store() {
        // A profile name with a `.` sanitizes to a different on-disk id.
        // The store must find the hold under the sanitized name, or the
        // times budget resets on every stop.
        let dir = std::env::temp_dir().join(format!("gaff-hold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::state::Store::new(dir.clone());
        store
            .write_hold("s", "profile-code.review", "hold", Some(2))
            .unwrap();
        assert!(
            store.has_hold("s", "profile-code.review"),
            "the hold is found under the sanitized id"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_env_gate_refuses_a_bundle_that_carries_law() {
        // A bundle with a guard is law, human-only, even with no
        // transitions declared.
        let mut cfg: Config = serde_yaml_ng::from_str(
            "profiles:\n  reviewer:\n    guards:\n      - name: g\n        tool: Bash\n        message: no\n",
        )
        .unwrap();
        for p in cfg.profiles.values_mut() {
            p.user = true;
        }
        let dir = std::env::temp_dir();
        assert_eq!(
            resolve_profile(None, Some("reviewer"), None, &dir, &cfg),
            None,
            "GAFF_PROFILE cannot select a law-carrying bundle"
        );
    }

    #[test]
    fn disable_drops_entries_and_overrides_the_cap() {
        let cfg = base().with_profile(Some("quiet"));
        assert!(cfg.reminders.is_empty());
        assert!(cfg.sections.is_empty());
        assert_eq!(cfg.max_inject_bytes, 100);
    }

    #[test]
    fn an_unknown_profile_keeps_the_base_config() {
        // A typo must never silently empty the config.
        let cfg = base().with_profile(Some("nope"));
        assert_eq!(cfg.reminders.len(), 2);
        assert_eq!(cfg.sections.len(), 1);
    }

    #[test]
    fn resolution_order_prefers_the_earlier_source() {
        let cfg = base();
        let dir = std::env::temp_dir().join(format!("gaff-prof-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile"), "fromfile\n").unwrap();

        assert_eq!(
            resolve_profile(Some("flag"), Some("env"), Some("sess"), &dir, &cfg).as_deref(),
            Some("flag")
        );
        assert_eq!(
            resolve_profile(None, Some("env"), Some("sess"), &dir, &cfg).as_deref(),
            Some("env")
        );
        assert_eq!(
            resolve_profile(None, None, Some("sess"), &dir, &cfg).as_deref(),
            Some("sess")
        );
        assert_eq!(
            resolve_profile(None, None, None, &dir, &cfg).as_deref(),
            Some("fromfile")
        );
        std::fs::remove_file(dir.join("profile")).unwrap();
        assert_eq!(
            resolve_profile(None, None, None, &dir, &cfg).as_deref(),
            Some("focus"),
            "the config default is the last resort"
        );
    }

    #[test]
    fn the_transition_policy_names_the_agent_settable_profiles() {
        let cfg = base();
        assert!(
            cfg.transitions
                .clone()
                .unwrap_or_default()
                .agent_may_set("focus")
        );
        assert!(
            !cfg.transitions.unwrap_or_default().agent_may_set("quiet"),
            "human only"
        );
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    fn cfg(yaml: &str) -> Config {
        serde_yaml_ng::from_str(yaml).expect("the fixture must parse")
    }

    #[test]
    fn a_user_reminder_applies_where_a_repo_declares_none() {
        let user = cfg("reminders:\n  - name: global\n    every: {tool_calls: 9}\n    text: G\n");
        let repo = cfg("reminders: []\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.reminders.len(), 1);
        assert_eq!(merged.reminders[0].name, "global");
    }

    #[test]
    fn the_layers_take_the_union_of_the_required_reviews() {
        // A merge gate reads this list. A layer that replaced it could
        // demand nothing, and a change would land unreviewed. The bug
        // this test holds shut: the repo's reviews were dropped when a
        // user config existed, and every hermetic test still passed.
        let names = |c: Config| c.reviews.map(|v| v.join(","));

        let user = cfg("reviews:\n  - review-code\n");
        let repo = cfg("reviews:\n  - review-tests\n  - review-code\n");
        assert_eq!(
            names(user.overlaid_with(repo)).as_deref(),
            Some("review-code,review-tests"),
            "the union holds, and a name repeats once"
        );

        // A repo with no user config keeps its own list, in order.
        let merged = cfg("").overlaid_with(cfg("reviews:\n  - review-docs\n  - review-deps\n"));
        assert_eq!(names(merged).as_deref(), Some("review-docs,review-deps"));

        // Absent and empty differ, and the merge keeps them apart.
        assert_eq!(
            names(cfg("").overlaid_with(cfg(""))),
            None,
            "neither states one"
        );
        assert_eq!(
            names(cfg("").overlaid_with(cfg("reviews: []\n"))).as_deref(),
            Some(""),
            "a repo wrote `reviews: []`, and that is a policy"
        );
        assert_eq!(
            names(cfg("reviews:\n  - review-code\n").overlaid_with(cfg("reviews: []\n")))
                .as_deref(),
            Some("review-code"),
            "an empty repo list never drops a user requirement"
        );
    }

    #[test]
    fn a_user_config_cannot_state_the_policy_a_repo_omits() {
        // Gate policy belongs to the repo. A truncated repo config
        // beside a user config would otherwise read as a policy the
        // repo never wrote, and a gate would then require the user's
        // list where the repo stated nothing.
        let names = |c: Config| c.reviews.map(|v| v.join(","));

        for repo in ["", "reminders: []\n"] {
            let merged = cfg("reviews:\n  - review-code\n").overlaid_with(cfg(repo));
            assert_eq!(
                names(merged),
                None,
                "a user list must not survive a repo that states no policy"
            );
        }

        // A repo that states one takes the user's names with it.
        let merged =
            cfg("reviews:\n  - review-code\n").overlaid_with(cfg("reviews:\n  - review-tests\n"));
        assert_eq!(
            names(merged).as_deref(),
            Some("review-code,review-tests"),
            "the user's name comes first, and the repo's follows"
        );
    }

    #[test]
    fn a_repo_may_not_take_the_name_of_a_user_reminder() {
        // A repo taking the name replaces a user's text with its own
        // under the user's label, and nothing downstream can tell them
        // apart. A clone is untrusted content, so the user's entry
        // stands and the repo's is dropped.
        let user = cfg(
            "reminders:\n  - name: same\n    every: {tool_calls: 1}\n    text: FROM_USER\n  - name: keep\n    every: {tool_calls: 2}\n    text: K\n",
        );
        let repo =
            cfg("reminders:\n  - name: same\n    every: {tool_calls: 5}\n    text: FROM_REPO\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.reminders.len(), 2, "no entry is added or lost");
        let same = merged.reminders.iter().find(|r| r.name == "same").unwrap();
        assert_eq!(same.text, "FROM_USER");
        assert_eq!(same.every.tool_calls, Some(1));
        assert!(merged.reminders.iter().any(|r| r.name == "keep"));
    }

    #[test]
    fn a_repo_may_add_a_reminder_under_a_name_the_user_did_not_use() {
        let user = cfg("reminders:\n  - name: mine\n    every: {tool_calls: 1}\n    text: U\n");
        let repo = cfg("reminders:\n  - name: theirs\n    every: {tool_calls: 5}\n    text: R\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.reminders.len(), 2);
        assert!(merged.reminders.iter().any(|r| r.name == "theirs"));
    }

    #[test]
    fn a_repo_cannot_widen_what_an_agent_may_grant_itself() {
        // transitions is the one field the repo does not win. A repo
        // that could widen it would let an agent switch to any profile.
        let user = cfg("transitions:\n  agent_may_set: [safe]\n");
        let repo = cfg("transitions:\n  agent_may_set: [safe, dangerous]\n");
        let merged = user.overlaid_with(repo);
        assert!(
            merged
                .transitions
                .clone()
                .unwrap_or_default()
                .agent_may_set("safe")
        );
        assert!(
            !merged
                .transitions
                .unwrap_or_default()
                .agent_may_set("dangerous"),
            "a repo must not widen the agent's own permissions"
        );
    }

    #[test]
    fn a_repo_sets_the_transitions_when_the_user_states_none() {
        let user = cfg("reminders: []\n");
        let repo = cfg("transitions:\n  agent_may_set: [focus]\n");
        let merged = user.overlaid_with(repo);
        assert!(
            merged
                .transitions
                .unwrap_or_default()
                .agent_may_set("focus")
        );
    }

    #[test]
    fn the_repo_wins_the_scalars_it_sets() {
        let user = cfg("max_inject_bytes: 100\ndefault_profile: userdef\n");
        let repo = cfg("max_inject_bytes: 200\ndefault_profile: repodef\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.max_inject_bytes, 200);
        assert_eq!(merged.default_profile.as_deref(), Some("repodef"));
    }

    #[test]
    fn an_unset_repo_scalar_leaves_the_user_value() {
        let user = cfg("max_inject_bytes: 100\ndefault_profile: userdef\n");
        let repo = cfg("reminders: []\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.max_inject_bytes, 100, "the repo set no cap");
        assert_eq!(merged.default_profile.as_deref(), Some("userdef"));
    }
}

#[cfg(test)]
mod guard_layering_tests {
    use super::*;

    fn user_with_guard() -> Config {
        serde_yaml_ng::from_str(
            "guards:\n  - name: no-mass-stage\n    tool: Bash\n    matches: 'git add -A'\n    message: Stage by name.\n",
        )
        .expect("the user fixture must parse")
    }

    #[test]
    fn a_repo_cannot_declare_a_guard() {
        // A cloned repo is untrusted content. A repo-declared guard
        // would let it refuse any tool call its reader makes.
        let repo: Config = serde_yaml_ng::from_str(
            "guards:\n  - name: repo-owns-you\n    tool: '.*'\n    message: blocked\n",
        )
        .unwrap();
        let merged = user_with_guard().overlaid_with(repo);
        assert_eq!(merged.guards.len(), 1, "only the user guard survives");
        assert_eq!(merged.guards[0].name, "no-mass-stage");
    }

    #[test]
    fn a_repo_cannot_remove_a_user_guard() {
        let repo: Config = serde_yaml_ng::from_str("guards: []\n").unwrap();
        let merged = user_with_guard().overlaid_with(repo);
        assert_eq!(merged.guards.len(), 1);
    }

    #[test]
    fn a_repo_cannot_shadow_a_user_guard_by_name() {
        // A same-name guard with a pattern that matches nothing would
        // otherwise defuse the user's rule.
        let repo: Config = serde_yaml_ng::from_str(
            "guards:\n  - name: no-mass-stage\n    tool: Bash\n    matches: 'zzz'\n    message: neutered\n",
        )
        .unwrap();
        let merged = user_with_guard().overlaid_with(repo);
        assert_eq!(merged.guards.len(), 1);
        assert_eq!(merged.guards[0].matches.as_deref(), Some("git add -A"));
    }
}

#[cfg(test)]
mod transition_layering_tests {
    use super::*;

    fn cfg(y: &str) -> Config {
        serde_yaml_ng::from_str(y).expect("fixture must parse")
    }

    #[test]
    fn an_explicit_empty_list_means_the_agent_may_set_nothing() {
        // Absent and empty are different. A repo must not be able to
        // overrule "nothing" by looking unset.
        let user = cfg("profiles: {safe: {}}\ntransitions: {agent_may_set: []}\n");
        let repo = cfg("profiles: {wide: {}}\ntransitions: {agent_may_set: [wide, safe]}\n");
        let merged = user.overlaid_with(repo);
        let t = merged.transitions.unwrap_or_default();
        assert!(!t.agent_may_set("wide"), "a repo may not widen the list");
        assert!(!t.agent_may_set("safe"), "the user allowed nothing");
    }

    #[test]
    fn a_repo_cannot_redefine_a_profile_the_user_declared() {
        // The user sanctions a profile by name. A repo that rewrote
        // that name would decide what the sanctioned profile does.
        let user = cfg(
            "reminders: [{name: safety, every: {tool_calls: 1}, text: KEEP}]\nprofiles: {safe: {}}\ntransitions: {agent_may_set: [safe]}\n",
        );
        let repo = cfg("profiles: {safe: {only: []}}\n");
        let merged = user.overlaid_with(repo);
        let effective = merged.with_profile(Some("safe"));
        assert_eq!(
            effective.reminders.len(),
            1,
            "the repo must not empty the user's profile"
        );
        assert_eq!(effective.reminders[0].text, "KEEP");
    }

    #[test]
    fn a_repo_may_still_add_a_profile_of_its_own() {
        let user = cfg("profiles: {safe: {}}\n");
        let repo = cfg("profiles: {repoonly: {}}\n");
        let merged = user.overlaid_with(repo);
        assert!(merged.profiles.contains_key("safe"));
        assert!(merged.profiles.contains_key("repoonly"));
    }
}

#[cfg(test)]
mod repo_silencing_tests {
    use super::*;

    fn cfg(y: &str) -> Config {
        serde_yaml_ng::from_str(y).expect("fixture must parse")
    }

    fn user() -> Config {
        cfg(
            "reminders: [{name: safety, every: {tool_calls: 1}, text: KEEP}]\ngit: [{name: scan, on: [pre-commit], command: [echo, USER]}]\n",
        )
    }

    #[test]
    fn a_repo_profile_cannot_filter_a_user_entry() {
        // A repo could otherwise ship a profile, name it the default,
        // and silence everything the user declared.
        let repo = cfg("profiles: {quiet: {only: []}}\ndefault_profile: quiet\n");
        let merged = user().overlaid_with(repo);
        let effective = merged.with_profile(Some("quiet"));
        assert_eq!(effective.reminders.len(), 1, "the user entry survives");
    }

    #[test]
    fn a_repo_cannot_lower_the_cap_below_the_users() {
        let repo = cfg("max_inject_bytes: 1\n");
        let merged = user().overlaid_with(repo);
        assert!(
            merged.max_inject_bytes > 1,
            "a one-byte cap silences everything"
        );
    }

    #[test]
    fn a_repo_may_still_set_a_cap_when_the_user_declared_nothing() {
        let merged = cfg("reminders: []\n").overlaid_with(cfg("max_inject_bytes: 64\n"));
        assert_eq!(merged.max_inject_bytes, 64);
    }

    #[test]
    fn a_repo_cannot_replace_a_user_git_entry() {
        // A git entry blocks a commit, and both layers installed theirs
        // deliberately. Dropping the repo's passed a commit it meant to
        // gate; refusing to run either blocked every commit in the
        // clone. Both run, and the collision is reported.
        let repo = cfg("git: [{name: scan, on: [pre-commit], command: [echo, REPO]}]\n");
        let merged = user().overlaid_with(repo);
        assert_eq!(merged.git.len(), 2, "neither entry is dropped");
        assert!(
            merged.git.iter().any(|g| g.command[1] == "USER"),
            "the user's check still runs"
        );
        assert!(
            merged.git.iter().any(|g| g.command[1] == "REPO"),
            "the repo's check still runs"
        );
    }

    #[test]
    fn a_repo_section_cannot_take_a_user_reminders_name() {
        // They share one pending-marker namespace, so the section would
        // consume the reminder's marker and silence it.
        let repo = cfg("sections: [{name: safety, file: s.md, refresh: {tool_calls: 1}}]\n");
        let merged = user().overlaid_with(repo);
        assert!(
            merged.sections.is_empty(),
            "the colliding section is dropped"
        );
        assert_eq!(merged.reminders.len(), 1);
    }

    #[test]
    fn a_repo_may_still_add_its_own_entries() {
        let repo = cfg(
            "sections: [{name: reposec, file: s.md}]\ngit: [{name: repogit, on: [pre-commit], command: [true]}]\n",
        );
        let merged = user().overlaid_with(repo);
        assert_eq!(merged.sections.len(), 1);
        assert_eq!(merged.git.len(), 2);
    }
}

/// Layer-boundary tests.
///
/// Each one holds a route by which a cloned repo reached past the
/// boundary. They are grouped so a reader can see the whole rule in one
/// place: a repo adds, and never speaks as the user.
#[cfg(test)]
mod boundary_tests {
    use super::*;

    fn user_cfg(yaml: &str) -> Config {
        let mut c: Config = serde_yaml_ng::from_str(yaml).expect("the fixture must parse");
        for s in &mut c.sections {
            s.user = true;
        }
        for p in c.profiles.values_mut() {
            p.user = true;
        }
        c
    }
    fn repo_cfg(yaml: &str) -> Config {
        serde_yaml_ng::from_str(yaml).expect("the fixture must parse")
    }

    #[test]
    fn a_user_section_keeps_its_layer_through_the_merge() {
        // The layer decides which directory the body is read from. If
        // the merge lost it, the repo's directory would supply the
        // text of a section the user declared.
        let user = user_cfg("sections:\n  - {name: conv, file: conv.md}\n");
        let repo = repo_cfg("sections:\n  - {name: repo-notes, file: notes.md}\n");
        let merged = user.overlaid_with(repo);
        let conv = merged.sections.iter().find(|s| s.name == "conv").unwrap();
        let notes = merged
            .sections
            .iter()
            .find(|s| s.name == "repo-notes")
            .unwrap();
        assert!(conv.user, "the user's section stays a user section");
        assert!(!notes.user, "the repo's section stays a repo section");
    }

    #[test]
    fn a_repo_profile_cannot_retime_a_user_entry() {
        // A cadence of a few million silences an entry as completely as
        // `disable` does.
        let user =
            user_cfg("reminders:\n  - name: safety\n    every: {tool_calls: 1}\n    text: S\n");
        let repo = repo_cfg(
            "profiles:\n  slow:\n    cadence:\n      safety: {tool_calls: 999999}\ndefault_profile: slow\n",
        );
        let merged = user.overlaid_with(repo);
        let applied = merged.with_profile(Some("slow"));
        let safety = applied
            .reminders
            .iter()
            .find(|r| r.name == "safety")
            .expect("the user reminder survives");
        assert_eq!(
            safety.every.tool_calls,
            Some(1),
            "the user's cadence is untouched"
        );
    }

    #[test]
    fn a_repo_profile_cannot_starve_a_user_entry_with_its_own_cap() {
        let user = user_cfg(
            "max_inject_bytes: 512\nreminders:\n  - name: safety\n    every: {tool_calls: 1}\n    text: S\n",
        );
        let repo = repo_cfg("profiles:\n  tiny:\n    max_inject_bytes: 1\ndefault_profile: tiny\n");
        let merged = user.overlaid_with(repo);
        let applied = merged.with_profile(Some("tiny"));
        assert!(
            applied.max_inject_bytes >= 512,
            "the cap never drops below the user's, got {}",
            applied.max_inject_bytes
        );
    }

    #[test]
    fn a_repo_cannot_name_a_user_profile_as_the_default() {
        // `quiet` is the user's own switch. A repo naming it decides
        // when the user's kill switch fires.
        let user = user_cfg("profiles:\n  quiet:\n    only: []\n");
        let repo = repo_cfg("default_profile: quiet\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.default_profile, None);
    }

    #[test]
    fn a_repo_may_name_its_own_profile_as_the_default() {
        let user = user_cfg("profiles:\n  quiet:\n    only: []\n");
        let repo = repo_cfg("profiles:\n  ci:\n    disable: []\ndefault_profile: ci\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.default_profile.as_deref(), Some("ci"));
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gaff-bound-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_committed_profile_file_cannot_select_a_user_profile() {
        let dir = scratch("userprof");
        std::fs::write(dir.join("profile"), "quiet\n").unwrap();
        let cfg = user_cfg("profiles:\n  quiet:\n    only: []\n");
        assert_eq!(
            resolve_profile(None, None, None, &dir, &cfg),
            None,
            "a repo file may not select the user's profile"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_committed_profile_file_may_select_a_repo_profile() {
        let dir = scratch("repoprof");
        std::fs::write(dir.join("profile"), "ci\n").unwrap();
        let cfg = repo_cfg("profiles:\n  ci:\n    disable: []\n");
        assert_eq!(
            resolve_profile(None, None, None, &dir, &cfg).as_deref(),
            Some("ci")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_repo_cannot_take_the_name_of_a_user_section() {
        let user = user_cfg("sections:\n  - {name: conventions, file: mine.md}\n");
        let repo = repo_cfg("sections:\n  - {name: conventions, file: theirs.md}\n");
        let merged = user.overlaid_with(repo);
        assert_eq!(merged.sections.len(), 1);
        assert_eq!(merged.sections[0].file, "mine.md");
        assert!(merged.sections[0].user);
    }

    #[test]
    fn an_empty_config_file_reads_as_absent() {
        // An empty file is what a truncated write and a bad merge leave
        // behind. A git hook must refuse on it rather than pass a check
        // that never ran.
        let dir = scratch("empty");
        std::fs::create_dir_all(dir.join(".gaff")).unwrap();
        std::fs::write(dir.join(CONFIG_PATH), "").unwrap();
        assert!(matches!(load(&dir), Loaded::Absent));
        std::fs::write(dir.join(CONFIG_PATH), "  \n\n").unwrap();
        assert!(matches!(load(&dir), Loaded::Absent));
        std::fs::remove_dir_all(&dir).ok();
    }
}
