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
}

/// A profile: a named overlay on reminders and sections.
///
/// A profile never adds an entry. It selects from the entries the base
/// config already declares, and it may override their cadences. That
/// keeps one namespace and one place to read what a repo can inject.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Keep only these entries. `None` keeps every entry.
    #[serde(default)]
    pub only: Option<Vec<String>>,
    /// Drop these entries. `disable` applies after `only`.
    #[serde(default)]
    pub disable: Vec<String>,
    /// Cadence overrides, keyed by the entry name.
    #[serde(default)]
    pub cadence: std::collections::BTreeMap<String, Every>,
    /// The byte cap under this profile. `None` keeps the base cap.
    #[serde(default)]
    pub max_inject_bytes: Option<usize>,
    /// True when the user config declared this profile. Set at load
    /// time, never read from YAML. A repo may not select a profile the
    /// user wrote for their own use.
    #[serde(skip)]
    pub user: bool,
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
        let keep = |n: &str| {
            profile
                .only
                .as_ref()
                .is_none_or(|only| only.iter().any(|k| k == n))
                && !profile.disable.iter().any(|d| d == n)
        };
        out.reminders.retain(|r| keep(&r.name));
        out.sections.retain(|s| keep(&s.name));
        for r in &mut out.reminders {
            if let Some(c) = profile.cadence.get(&r.name) {
                r.every = c.clone();
            }
        }
        for s in &mut out.sections {
            if let Some(c) = profile.cadence.get(&s.name) {
                s.refresh = c.clone();
            }
        }
        if let Some(cap) = profile.max_inject_bytes {
            out.max_inject_bytes = cap;
        }
        out
    }
}

/// Strip a repo profile of everything that would reach a user entry.
///
/// A profile filters, retimes, and caps. Each of those silences an
/// entry as completely as the others, so all four fields are held to
/// one rule: a repo profile governs the repo's own entries and nothing
/// the user declared.
fn sanitize_repo_profile(mut profile: Profile, user_entries: &[String], user_cap: usize) -> Profile {
    if let Some(only) = &mut profile.only {
        only.extend(user_entries.iter().cloned());
    }
    profile.disable.retain(|d| !user_entries.contains(d));
    profile.cadence.retain(|k, _| !user_entries.contains(k));
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
    let meta = if section.user {
        std::fs::metadata(&path)
    } else {
        let raw = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("section `{}`: cannot read {}: {e}", section.name, path.display()))?;
        if raw.file_type().is_symlink() {
            return Err(format!(
                "section `{}`: {} is a symlink. A repo section reads a regular file only, because a link can point at any file gaff can read.",
                section.name,
                path.display()
            ));
        }
        Ok(raw)
    }
    .map_err(|e| format!("section `{}`: cannot read {}: {e}", section.name, path.display()))?;
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
    std::fs::read_to_string(&path)
        .map_err(|e| format!("section `{}`: cannot read {}: {e}", section.name, path.display()))
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
                    }
                    for g in &mut cfg.git {
                        g.user = true;
                    }
                    for w in &mut cfg.github {
                        w.user = true;
                    }
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
        (Some(user), Loaded::Absent) => Loaded::Ok(user),
        (Some(user), Loaded::Ok(repo) | Loaded::Degraded(repo)) => {
            Loaded::Ok(user.overlaid_with(repo))
        }
        // A repo that cannot be parsed contributes nothing. It must
        // not delete what the user declared, because the user's guards
        // are refusals and a repo could otherwise switch them all off
        // with one bad line.
        (Some(user), Loaded::Broken(err)) => {
            eprintln!("gaff: {CONFIG_PATH} is not valid: {err}. Using the user config alone.");
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
        // A repo may add a git entry or a workflow, and it may not
        // replace one the user declared. Both run commands, and the
        // consent a user gave was to their own entry, not to whatever
        // a later pull puts under the same name.
        for entry in repo.git {
            if self.git.iter().any(|u| u.name == entry.name) {
                eprintln!(
                    "gaff: the repo declares a git entry named `{}`, which the user declared. Keeping the user's.",
                    entry.name
                );
                continue;
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
            "reminders:\n  - name: a\n    every: {tool_calls: 5}\n    text: A\n  - name: b\n    every: {tool_calls: 7}\n    text: B\nsections:\n  - name: s\n    file: s.md\n    refresh: {prompts: 3}\nprofiles:\n  focus:\n    only: [a]\n    cadence:\n      a: {tool_calls: 2}\n  quiet:\n    disable: [a, b, s]\n    max_inject_bytes: 100\ndefault_profile: focus\ntransitions:\n  agent_may_set: [focus]\n",
        )
        .expect("the fixture config must parse")
    }

    #[test]
    fn only_selects_and_cadence_overrides() {
        let cfg = base().with_profile(Some("focus"));
        assert_eq!(cfg.reminders.len(), 1, "only: [a] keeps one reminder");
        assert_eq!(cfg.reminders[0].name, "a");
        assert_eq!(
            cfg.reminders[0].every.tool_calls,
            Some(2),
            "the profile overrides the cadence"
        );
        assert!(cfg.sections.is_empty(), "only: [a] drops the section");
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
        assert!(cfg
            .transitions
            .clone()
            .unwrap_or_default()
            .agent_may_set("focus"));
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
    fn a_repo_may_not_take_the_name_of_a_user_reminder() {
        // A repo taking the name replaces a user's text with its own
        // under the user's label, and nothing downstream can tell them
        // apart. A clone is untrusted content, so the user's entry
        // stands and the repo's is dropped.
        let user = cfg("reminders:\n  - name: same\n    every: {tool_calls: 1}\n    text: FROM_USER\n  - name: keep\n    every: {tool_calls: 2}\n    text: K\n");
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
        assert!(merged
            .transitions
            .clone()
            .unwrap_or_default()
            .agent_may_set("safe"));
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
        assert!(merged
            .transitions
            .unwrap_or_default()
            .agent_may_set("focus"));
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
        let user = cfg("reminders: [{name: safety, every: {tool_calls: 1}, text: KEEP}]\nprofiles: {safe: {}}\ntransitions: {agent_may_set: [safe]}\n");
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
        cfg("reminders: [{name: safety, every: {tool_calls: 1}, text: KEEP}]\ngit: [{name: scan, on: [pre-commit], command: [echo, USER]}]\n")
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
        let repo = cfg("git: [{name: scan, on: [pre-commit], command: [echo, REPO]}]\n");
        let merged = user().overlaid_with(repo);
        assert_eq!(merged.git.len(), 1);
        assert_eq!(merged.git[0].command[1], "USER");
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
        let repo = cfg("sections: [{name: reposec, file: s.md}]\ngit: [{name: repogit, on: [pre-commit], command: [true]}]\n");
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
        let user = user_cfg("reminders:\n  - name: safety\n    every: {tool_calls: 1}\n    text: S\n");
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
