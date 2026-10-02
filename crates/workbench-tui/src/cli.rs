use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use workbench_core::{WorkItem, WorkItemKind, default_state_file};

pub(crate) const HELP: &str = "Agent Workbench — M6 local diff review

Usage:
  workbench [--state-file PATH] [--diff-base REV]
  workbench register --id ID --kind implementation|external-review
      --repository PATH --workspace PATH --pane %ID [--short-description TEXT] [--branch NAME]
      [--state-file PATH]
  workbench list [--state-file PATH]
  workbench --help

The TUI defaults to ATTENTION; a returns there, w shows all work, s shows sessions.
In ATTENTION, x acknowledges the selected TURN FINISHED observation for this
Workbench run. New activity/input or changed completion evidence requeues it.
Acknowledgement does not finish work, mark code reviewed, or dismiss input requests.
In REVIEW, Space saves file marks; r reloads while restoring unchanged marks.
Marks persist across restarts; changed captured diffs require re-review.
In SESSIONS, j/k or arrows select a pane; <enter> or r opens registration with
pane/Git metadata filled in. <enter> confirms; Tab changes fields; Esc cancels.
SESSIONS defaults to recognized agent commands; f toggles all panes.
In WORK, e edits Short Description; u opens unregister confirmation. Unregistering
removes only the Workbench entry, keeping pane, branch, worktree and review history.
Short descriptions are limited to 120 Unicode characters; --title is a legacy
alias for --short-description. When omitted, the description defaults to the ID.
j/k or arrows select; <enter> opens the pane; r composes a single-line reply.
<c-d>/<c-u> scroll down/up half a page; Page Down/Page Up scroll a full page.
<enter> submits a reply and Esc cancels. Tab selects known waiting/completed items.
While composing a reply, <c-u> clears the draft instead of scrolling.
d opens local diff review; j/k select files, Space toggles reviewed, Esc returns.
Review defaults to HEAD (staged, unstaged, and untracked changes). --diff-base REV
compares the working tree directly to that local commit/ref, including committed
changes. No fetching or inferred merge-base. Review marks are in-memory only.
In review, r reloads and clears marks; h/l pan long lines; <c-d>/<c-u> scroll.
Registration does not require a running tmux server. Relative repository/workspace
paths are resolved from the current directory; expand ~ using your shell.
State: --state-file, then AGENT_WORKBENCH_STATE_FILE, then
$XDG_STATE_HOME/agent-workbench/work-items.json or
$HOME/.local/state/agent-workbench/work-items.json.
";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Run,
    Register(WorkItem),
    List,
    Help,
}

#[derive(Debug)]
pub(crate) struct Options {
    pub state_file: Option<PathBuf>,
    pub diff_base: Option<String>,
    pub command: Command,
}

impl Options {
    pub fn state_path(&self) -> Result<PathBuf, String> {
        let path = match &self.state_file {
            Some(path) => path.clone(),
            None => default_state_file().map_err(|error| error.to_string())?,
        };
        absolute(path)
    }
}

fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || path.starts_with("~") {
        return Err("Paths must be nonempty; expand ~ using your shell.".into());
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut args = args.into_iter();
    let mut command = None;
    let mut flags = BTreeMap::new();
    while let Some(argument) = args.next() {
        let flag = argument
            .to_str()
            .ok_or("Commands and flag names must be UTF-8.")?;
        if matches!(flag, "--help" | "-h") {
            return Ok(Options {
                state_file: None,
                diff_base: None,
                command: Command::Help,
            });
        }
        if matches!(flag, "register" | "list") && command.is_none() {
            command = Some(flag.to_owned());
            continue;
        }
        if !matches!(
            flag,
            "--state-file"
                | "--diff-base"
                | "--id"
                | "--title"
                | "--short-description"
                | "--kind"
                | "--repository"
                | "--workspace"
                | "--pane"
                | "--branch"
        ) {
            return Err(format!("Unknown argument {flag:?}. See --help."));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value for {flag}."))?;
        if value.is_empty() || value.to_str().is_some_and(|value| value.starts_with("--")) {
            return Err(format!("Missing value for {flag}."));
        }
        if flags.insert(flag.to_owned(), value).is_some() {
            return Err(format!("Duplicate flag {flag}."));
        }
    }
    let state_file = flags.remove("--state-file").map(PathBuf::from);
    let command = match command.as_deref() {
        Some("register") => {
            let id = string_flag(&mut flags, "--id")?;
            let kind = match string_flag(&mut flags, "--kind")?.as_str() {
                "implementation" => WorkItemKind::Implementation,
                "external-review" => WorkItemKind::ExternalReview,
                _ => return Err("--kind must be implementation or external-review.".into()),
            };
            let repository = absolute(PathBuf::from(required(&mut flags, "--repository")?))?;
            let workspace = absolute(PathBuf::from(required(&mut flags, "--workspace")?))?;
            let pane_id = string_flag(&mut flags, "--pane")?;
            let description = optional_string(&mut flags, "--short-description")?;
            let legacy_title = optional_string(&mut flags, "--title")?;
            if description.is_some() && legacy_title.is_some() {
                return Err("Use --short-description or --title, not both.".into());
            }
            let title = description.or(legacy_title).unwrap_or_else(|| id.clone());
            let branch = optional_string(&mut flags, "--branch")?;
            Command::Register(WorkItem {
                id,
                title,
                repository,
                workspace,
                branch,
                kind,
                pane_id,
            })
        }
        Some("list") => Command::List,
        None => Command::Run,
        _ => unreachable!("command was checked while parsing"),
    };
    let diff_base = if command == Command::Run {
        optional_string(&mut flags, "--diff-base")?
    } else {
        None
    };
    if diff_base
        .as_ref()
        .is_some_and(|base| base.chars().any(char::is_control))
    {
        return Err("--diff-base must not contain control characters.".into());
    }
    if let Some(flag) = flags.keys().next() {
        return Err(format!(
            "{flag} is not valid with this command. See --help."
        ));
    }
    Ok(Options {
        state_file,
        diff_base,
        command,
    })
}

fn required(flags: &mut BTreeMap<String, OsString>, name: &str) -> Result<OsString, String> {
    flags
        .remove(name)
        .ok_or_else(|| format!("Missing required flag {name}. See --help."))
}

fn string_flag(flags: &mut BTreeMap<String, OsString>, name: &str) -> Result<String, String> {
    required(flags, name)?
        .into_string()
        .map_err(|_| format!("{name} must be UTF-8."))
}

fn optional_string(
    flags: &mut BTreeMap<String, OsString>,
    name: &str,
) -> Result<Option<String>, String> {
    flags
        .remove(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| format!("{name} must be UTF-8."))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &[&str]) -> Vec<OsString> {
        text.iter().map(OsString::from).collect()
    }

    #[test]
    fn short_description_flag_and_legacy_alias_are_mutually_exclusive() {
        let base = args(&[
            "register",
            "--id",
            "ABC",
            "--kind",
            "implementation",
            "--repository",
            "/work",
            "--workspace",
            "/work",
            "--pane",
            "%1",
        ]);
        for flag in ["--short-description", "--title"] {
            let mut argv = base.clone();
            argv.extend(args(&[flag, "Fix retries"]));
            let Command::Register(item) = parse(argv).unwrap().command else {
                panic!("expected registration")
            };
            assert_eq!(item.title, "Fix retries");
        }
        let mut both = base.clone();
        both.extend(args(&["--short-description", "First", "--title", "Second"]));
        assert!(parse(both).unwrap_err().contains("not both"));
        assert!(parse(args(&["list", "--short-description", "unused"])).is_err());
        let mut duplicate = base;
        duplicate.extend(args(&[
            "--short-description",
            "First",
            "--short-description",
            "Second",
        ]));
        assert!(parse(duplicate).unwrap_err().contains("Duplicate flag"));
    }

    #[test]
    fn parses_both_kinds_and_resolves_paths_without_requiring_them_to_exist() {
        for (kind, expected) in [
            ("implementation", WorkItemKind::Implementation),
            ("external-review", WorkItemKind::ExternalReview),
        ] {
            let options = parse(args(&[
                "--state-file",
                "state.json",
                "register",
                "--id",
                "PR #1842",
                "--kind",
                kind,
                "--repository",
                ".",
                "--workspace",
                "workspace with spaces",
                "--pane",
                "%14",
                "--branch",
                "fix/retry",
            ]))
            .unwrap();
            let Command::Register(item) = options.command else {
                panic!("not a registration")
            };
            assert_eq!(item.kind, expected);
            assert_eq!(item.title, "PR #1842");
            assert!(item.repository.is_absolute());
            assert!(item.workspace.ends_with("workspace with spaces"));
            assert_eq!(item.branch.as_deref(), Some("fix/retry"));
        }
    }

    #[test]
    fn diff_base_is_optional_and_only_applies_to_the_tui() {
        assert_eq!(parse(args(&[])).unwrap().diff_base, None);
        let options = parse(args(&[
            "--diff-base",
            "origin/main",
            "--state-file",
            "items.json",
        ]))
        .unwrap();
        assert_eq!(options.diff_base.as_deref(), Some("origin/main"));
        assert_eq!(options.command, Command::Run);
        for argv in [
            args(&["--diff-base"]),
            args(&["--diff-base", "a", "--diff-base", "b"]),
            args(&["list", "--diff-base", "HEAD"]),
            args(&["--diff-base", "bad\nref"]),
        ] {
            assert!(parse(argv).is_err());
        }
    }

    #[test]
    fn rejects_unknown_duplicate_missing_and_inapplicable_flags() {
        for argv in [
            args(&["register"]),
            args(&["--state-file"]),
            args(&["--unknown"]),
            args(&["--state-file", "a", "--state-file", "b"]),
            args(&["list", "--id", "ABC-123"]),
            args(&["--id", "ABC-123"]),
            args(&["register", "--id", "--kind", "implementation"]),
            args(&["register", "--id", "ABC-123", "--kind", "other"]),
        ] {
            assert!(parse(argv.clone()).is_err(), "accepted {argv:?}");
        }
        assert_eq!(parse(args(&["--help"])).unwrap().command, Command::Help);
        assert_eq!(parse(args(&[])).unwrap().command, Command::Run);
    }
}
