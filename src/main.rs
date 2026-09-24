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

fn load_config() -> Config {
    let config_path = PathBuf::from(std::env::var("HOME").expect("HOME not set"))
        .join(".config/tmux-booster/config.toml");

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

  CLI args are merged with the config file. Duplicates are removed.
  See CONFIG.md for full documentation."
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

fn expand_tilde(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").expect("HOME not set");
        format!("{}/{}", home, stripped)
    } else {
        path.to_string()
    }
}

fn get_project_directories(directories: &[String]) -> Vec<PathBuf> {
    directories
        .iter()
        .map(|directory| PathBuf::from(expand_tilde(directory)))
        .collect()
}

fn get_directories(directories: &[String]) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths = vec![];
    for directory in directories {
        let expanded = expand_tilde(directory);
        let entries = fs::read_dir(&expanded).map_err(|e| format!("{} {}", e, expanded))?; // better error message
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
    Command::new("tmux").args(args).status()?;
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

fn display_options_from_options(
    options: Vec<String>,
    live_sessions: &[String],
    attach_session_name: &str,
) -> Vec<String> {
    options
        .into_iter()
        .map(|r| {
            let target = tmux_target_name(&r);
            // Force styling: console disables colors when stdout isn't a tty,
            // but these strings are fed to the picker, not printed directly.
            if attach_session_name == target {
                style(r).yellow().force_styling(true).to_string()
            } else if live_sessions.contains(&target) {
                style(r).green().force_styling(true).to_string()
            } else {
                r
            }
        })
        .collect()
}

fn select_with_tv(items: Vec<String>) -> Option<String> {
    let mut child = Command::new("tv")
        .arg("--ansi")
        .arg("--source-command")
        .arg("echo placeholder") // we'll override via stdin piping trick
        .arg("--input-header")
        .arg("Projects")
        .arg("--no-status-bar")
        .arg("--no-remote")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to start tv");

    // write items to stdin
    if let Some(mut stdin) = child.stdin.take() {
        for item in &items {
            writeln!(stdin, "{}", item).ok();
        }
    }

    let output = child.wait_with_output().expect("failed to wait on tv");
    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

// Mirrors tv's default look: rounded border, prompt-on-top layout, and tv's
// palette re-created from terminal-native ANSI indices (8 gray, 2 green,
// 9 red, 10 bright green, 12 bright blue) instead of skim's own theme.
const TV_LIKE_COLORS: &str =
    "border:8,header:2:bold,prompt:9:bold,info:9:italic,fg+:10,bg+:8,hl+:10,hl:12,normal:12,cursor:-1,selected:-1";

fn select_with_skim(items: Vec<String>) -> Option<String> {
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

    let project_paths = get_project_directories(&projects);
    let project_dir_paths = get_directories(&directories)?;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut entries: Vec<(String, PathBuf)> = project_paths
        .into_iter()
        .chain(project_dir_paths)
        .filter(|p| seen.insert(p.clone()))
        .map(|p| (project_name(&p), p))
        .collect();
    entries.sort_by_key(|(name, _)| name.to_lowercase());

    let live_sessions = tmux_list_sessions()?;
    let attach_session_name = tmux_attached_session_name()?;
    let names: Vec<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    let display_options = display_options_from_options(names, &live_sessions, &attach_session_name);
    let selection = if use_tv {
        select_with_tv(display_options)
    } else {
        select_with_skim(display_options)
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
