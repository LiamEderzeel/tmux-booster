use clap::Parser;
use console::{strip_ansi_codes, style};
use serde::Deserialize;
use skim::prelude::{Skim, SkimItemReader, SkimItemReaderOption, SkimOptionsBuilder};
use skim::tui::options::TuiLayout;
use skim::tui::statusline::InfoDisplay;
use skim::tui::BorderType;
use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

#[derive(Deserialize, Debug, Default)]
struct Config {
    #[serde(default)]
    directory_paths: Vec<String>,
    #[serde(default)]
    project_paths: Vec<String>,
    #[serde(default)]
    tv: bool,
}

fn home_dir() -> PathBuf {
    PathBuf::from(env::var_os("HOME").expect("HOME not set"))
}

fn load_config() -> Config {
    let config_path = home_dir().join(".config/tmux-booster/config.toml");

    let Ok(contents) = fs::read_to_string(&config_path) else {
        return Config::default();
    };

    toml::from_str(&contents).unwrap_or_else(|e| {
        eprintln!("Warning: failed to parse config file: {}", e);
        Config::default()
    })
}

#[derive(Parser)]
#[command(
    author,
    about = "A tmux session manager",
    long_about = "A tmux session manager.

Configuration file:
  tmux-booster can be configured via ~/.config/tmux-booster/config.toml

  Example config:

    directory_paths = [
        \"~/projects\",
        \"~/work\",
    ]

    project_paths = [
        \"~/dotfiles\",
        \"~/projects/my-app\",
    ]

    # use the external tv binary instead of the embedded picker
    tv = true

  CLI args are merged with the config file. Duplicates are removed.
  tv is enabled if either the config or --tv sets it.
  See README.md for full documentation."
)]
struct Cli {
    #[arg(
        short = 'd',
        help = "path or paths to directory containing project directories."
    )]
    project_directories: Vec<String>,

    #[arg(short = 'p', help = "path or paths to project directory.")]
    projects: Vec<String>,

    #[arg(
        long,
        help = "use the external tv binary instead of the embedded skim picker"
    )]
    tv: bool,
}

fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(stripped) => home_dir().join(stripped),
        None => PathBuf::from(path),
    }
}

fn expand_paths(paths: &[String]) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    paths
        .iter()
        .map(|path| {
            let expanded = expand_tilde(path);
            let metadata = fs::metadata(&expanded).map_err(|e| format!("{} {}", e, expanded.display()))?;
            if !metadata.is_dir() {
                return Err(format!("Not a directory {}", expanded.display()).into());
            }
            Ok(expanded)
        })
        .collect()
}

fn list_subdirectories(directories: &[String]) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths = vec![];
    for directory in directories {
        let expanded = expand_tilde(directory);
        let entries = fs::read_dir(&expanded).map_err(|e| format!("{} {}", e, expanded.display()))?; // better error message
        paths.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir()),
        );
    }
    Ok(paths)
}

fn tmux_attached_session_name() -> Result<String, Box<dyn Error>> {
    // tmux display-message -p '#S'
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#S"])
        .output()?;

    let raw_output = String::from_utf8_lossy(&output.stdout);
    Ok(raw_output.trim_end_matches(['\r', '\n']).to_string())
}

fn tmux_is_attached() -> bool {
    env::var_os("TMUX").is_some()
}

fn tmux_list_sessions() -> Result<Vec<String>, Box<dyn Error>> {
    let output = Command::new("tmux")
        .args(["list-session", "-F", "#S"])
        .output()?;

    let raw_output = String::from_utf8_lossy(&output.stdout);
    let res = raw_output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<String>>();

    Ok(res)
}

fn tmux_target_name(name: &str) -> String {
    // tmux uses '.' as the session:window.pane separator, so a literal dot in
    // a session name breaks target lookups (e.g. switch/attach) even though
    // session creation accepts it. Sanitize consistently everywhere a tmux
    // session name is created or targeted.
    name.replace('.', "_")
}

fn run_tmux<I, S>(args: I) -> io::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("tmux");
    command.args(args);
    let status = command.status()?;
    if !status.success() {
        return Err(io::Error::other(format!("{:?} failed: {}", command, status)));
    }
    Ok(())
}

fn tmux_create_session(name: &str, path: &Path) -> io::Result<()> {
    let tmux_name = tmux_target_name(name);
    run_tmux([
        OsStr::new("new-session"),
        OsStr::new("-ds"),
        OsStr::new(&tmux_name),
        OsStr::new("-c"),
        path.as_os_str(),
    ])
}

fn tmux_switch_session(name: &str) -> io::Result<()> {
    run_tmux(["switch", "-t", &tmux_target_name(name)])
}

fn tmux_attach_session(name: &str) -> io::Result<()> {
    run_tmux(["attach", "-t", &tmux_target_name(name)])
}

fn project_name(path: &Path) -> String {
    let file_name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let parent = path.parent().map(file_name).unwrap_or_default();
    format!("{}/{}", parent, file_name(path))
}

fn colorize_names(
    names: Vec<String>,
    live_sessions: &[String],
    attach_session_name: &str,
) -> Vec<String> {
    names
        .into_iter()
        .map(|name| {
            let target = tmux_target_name(&name);
            // Force styling: console disables colors when stdout isn't a tty,
            // but these strings are fed to the picker, not printed directly.
            if attach_session_name == target {
                style(name).yellow().force_styling(true).to_string()
            } else if live_sessions.contains(&target) {
                style(name).green().force_styling(true).to_string()
            } else {
                name
            }
        })
        .collect()
}

fn select_with_tv(items: &[String]) -> Result<Option<String>, Box<dyn Error>> {
    // tv needs a --source-command for --ansi and friends to be accepted in
    // ad-hoc mode, but piped stdin takes precedence, so the command never runs.
    let mut child = Command::new("tv")
        .arg("--ansi")
        .arg("--source-command")
        .arg("echo placeholder")
        .arg("--input-header")
        .arg("Projects")
        .arg("--no-status-bar")
        .arg("--no-remote")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start tv (is it installed and on PATH?): {}", e))?;

    // write items to stdin
    if let Some(mut stdin) = child.stdin.take() {
        for item in items {
            writeln!(stdin, "{}", item).ok();
        }
    }

    let output = child.wait_with_output()?;
    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok((!result.is_empty()).then_some(result))
}

// Mirrors tv's default look: rounded border, prompt-on-top layout, and tv's
// palette re-created from terminal-native ANSI indices (8 gray, 2 green,
// 9 red, 10 bright green, 12 bright blue) instead of skim's own theme.
const TV_LIKE_COLORS: &str =
    "border:8,header:2:bold,prompt:9:bold,info:9:italic,fg+:10,bg+:8,hl+:10,hl:12,normal:12,cursor:-1,selected:-1";

fn select_with_skim(items: &[String]) -> Option<String> {
    let options = SkimOptionsBuilder::default()
        .header("Projects")
        .border(BorderType::Rounded)
        .layout(TuiLayout::Reverse)
        .info(InfoDisplay::InlineRight)
        .color(TV_LIKE_COLORS)
        .build()
        .expect("failed to build skim options");

    let reader_option = SkimItemReaderOption::default().ansi(true).build();
    let source = SkimItemReader::new(reader_option).of_bufread(Cursor::new(items.join("\n").into_bytes()));

    let output = Skim::run_with(options, Some(source)).ok()?;

    if output.is_abort {
        return None;
    }

    output
        .selected_items
        .first()
        .map(|item| item.output().to_string())
}

// Returns (name, path) pairs for every project, deduplicated by path and
// sorted case-insensitively by name.
fn collect_projects(
    directories: &[String],
    projects: &[String],
) -> Result<Vec<(String, PathBuf)>, Box<dyn Error>> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut entries: Vec<(String, PathBuf)> = expand_paths(projects)?
        .into_iter()
        .chain(list_subdirectories(directories)?)
        .filter(|p| seen.insert(p.clone()))
        .map(|p| (project_name(&p), p))
        .collect();
    entries.sort_by_key(|(name, _)| name.to_lowercase());
    Ok(entries)
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let config = load_config();

    // Bool flags: true in either config or CLI wins
    let use_tv = config.tv || cli.tv;

    // Merge: config provides the base, CLI args are appended
    let directories: Vec<String> = config
        .directory_paths
        .into_iter()
        .chain(cli.project_directories)
        .collect();

    let projects: Vec<String> = config
        .project_paths
        .into_iter()
        .chain(cli.projects)
        .collect();

    let entries = collect_projects(&directories, &projects)?;

    let live_sessions = tmux_list_sessions()?;
    let attach_session_name = tmux_attached_session_name()?;
    let names: Vec<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    let display_options = colorize_names(names, &live_sessions, &attach_session_name);
    let selection = if use_tv {
        select_with_tv(&display_options)?
    } else {
        select_with_skim(&display_options)
    };
    let Some(selection) = selection else {
        eprintln!("no selection made");
        std::process::exit(1);
    };

    let clean_selection = strip_ansi_codes(&selection);

    let Some((project_name, project_path)) =
        entries.iter().find(|(name, _)| *name == clean_selection)
    else {
        eprintln!("no index found for selected option");
        std::process::exit(1)
    };

    let is_attached = tmux_is_attached();

    if !live_sessions.contains(&tmux_target_name(project_name)) {
        tmux_create_session(project_name, project_path)?;
    }

    if is_attached {
        tmux_switch_session(project_name)?;
    } else {
        tmux_attach_session(project_name)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_uses_parent_and_file_name() {
        assert_eq!(project_name(Path::new("/home/me/projects/app")), "projects/app");
        assert_eq!(project_name(Path::new("/home/me/dotfiles/")), "me/dotfiles");
    }

    #[test]
    fn project_name_does_not_panic_near_root() {
        assert_eq!(project_name(Path::new("/foo")), "/foo");
        assert_eq!(project_name(Path::new("/")), "/");
    }

    #[test]
    fn expand_tilde_replaces_home_prefix() {
        assert_eq!(expand_tilde("~/projects"), home_dir().join("projects"));
    }

    #[test]
    fn expand_tilde_leaves_other_paths_alone() {
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("relative"), PathBuf::from("relative"));
        assert_eq!(expand_tilde("~"), PathBuf::from("~"));
    }

    #[test]
    fn tmux_target_name_replaces_dots() {
        assert_eq!(tmux_target_name("work/my.app"), "work/my_app");
        assert_eq!(tmux_target_name("work/app"), "work/app");
    }

    #[test]
    fn colorize_names_marks_attached_and_live_sessions() {
        let names = vec![
            "a/attached".to_string(),
            "a/live.dot".to_string(),
            "a/idle".to_string(),
        ];
        let live = vec!["a/attached".to_string(), "a/live_dot".to_string()];

        let colored = colorize_names(names.clone(), &live, "a/attached");

        assert_eq!(colored[0], style("a/attached").yellow().force_styling(true).to_string());
        assert_eq!(colored[1], style("a/live.dot").green().force_styling(true).to_string());
        assert_eq!(colored[2], "a/idle");
        assert_ne!(colored[0], names[0]);
        assert_ne!(colored[1], names[1]);
        // The selection is matched back to a project after stripping colors.
        for (colored, name) in colored.iter().zip(&names) {
            assert_eq!(strip_ansi_codes(colored), name.as_str());
        }
    }

    // Creates an empty, unique scratch directory under the system temp dir.
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("tmux-booster-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collect_projects_merges_dedupes_and_sorts() {
        let root = scratch_dir("collect");
        fs::create_dir_all(root.join("dirs/Beta")).unwrap();
        fs::create_dir_all(root.join("dirs/alpha")).unwrap();
        fs::write(root.join("dirs/not-a-dir.txt"), "").unwrap();
        fs::create_dir_all(root.join("other/proj")).unwrap();

        let directories = vec![root.join("dirs").to_string_lossy().into_owned()];
        let projects = vec![
            root.join("other/proj").to_string_lossy().into_owned(),
            // Also found via `directories`; should only appear once.
            root.join("dirs/alpha").to_string_lossy().into_owned(),
        ];

        let entries = collect_projects(&directories, &projects).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            entries,
            vec![
                ("dirs/alpha".to_string(), root.join("dirs/alpha")),
                ("dirs/Beta".to_string(), root.join("dirs/Beta")),
                ("other/proj".to_string(), root.join("other/proj")),
            ]
        );
    }

    #[test]
    fn collect_projects_errors_on_missing_directory() {
        let root = scratch_dir("missing");
        let missing = root.join("does-not-exist").to_string_lossy().into_owned();
        fs::remove_dir_all(&root).unwrap();

        assert!(collect_projects(&[missing], &[]).is_err());
    }

    #[test]
    fn collect_projects_errors_on_missing_project() {
        let root = scratch_dir("missing-project");
        let missing = root.join("does-not-exist").to_string_lossy().into_owned();
        fs::remove_dir_all(&root).unwrap();

        assert!(collect_projects(&[], &[missing]).is_err());
    }

    #[test]
    fn collect_projects_errors_on_project_that_is_a_file() {
        let root = scratch_dir("file-project");
        let file = root.join("file.txt");
        fs::write(&file, "").unwrap();

        let result = collect_projects(&[], &[file.to_string_lossy().into_owned()]);
        fs::remove_dir_all(&root).unwrap();

        assert!(result.is_err());
    }
}
