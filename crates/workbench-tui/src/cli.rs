use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use workbench_core::{WorkItem, WorkItemKind, default_state_file};

pub(crate) const HELP: &str = "Agent Workbench — M3 navigation and input

Usage:
  workbench [--state-file PATH]
  workbench register --id ID --kind implementation|external-review
      --repository PATH --workspace PATH --pane %ID [--title TITLE] [--branch NAME]
      [--state-file PATH]
  workbench list [--state-file PATH]
  workbench --help

The TUI defaults to WORK; press s for sessions and w for work items.
j/k or arrows select; Enter opens the pane; r composes a single-line reply.
Enter submits a reply and Esc cancels. Tab reports known attention items (none yet).
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
                | "--id"
                | "--title"
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
            let title = optional_string(&mut flags, "--title")?.unwrap_or_else(|| id.clone());
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
    if let Some(flag) = flags.keys().next() {
        return Err(format!("{flag} is only valid with register."));
    }
    Ok(Options {
        state_file,
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
