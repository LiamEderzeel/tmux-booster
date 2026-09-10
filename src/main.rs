use clap::Parser;
use serde::Deserialize;
use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

#[derive(Deserialize, Debug, Default)]
struct Config {
    #[serde(default)]
    directory_paths: Vec<String>,
    #[serde(default)]
    project_paths: Vec<String>,
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
}

fn expand_tilde(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").expect("HOME not set");
        format!("{}/{}", home, stripped)
    } else {
        path.to_string()
    }
}

fn get_project_directories(directories: Vec<String>) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths: Vec<PathBuf> = vec![];

    for directory in &directories {
        let expanded = expand_tilde(directory);
        paths.push(PathBuf::from(expanded));
    }
    Ok(paths)
}

fn get_directories(directories: Vec<String>) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths: Vec<Vec<PathBuf>> = vec![];
    for directory in &directories {
        let expanded = expand_tilde(directory);
        let res = fs::read_dir(Path::new(&expanded)).map_err(|e| format!("{} {}", e, expanded))?; // better error message
        paths.push(
            res.into_iter()
                .filter(|r| r.is_ok())
                .map(|r| r.unwrap().path())
                .filter(|r| r.is_dir())
                .collect(),
        );
    }
    Ok(paths.into_iter().flatten().collect())
}

fn tmux_attached_session_name() -> Result<String, Box<dyn Error>> {
    // tmux display-message -p '#S'
    let output = Command::new("tmux")
        .arg("display-message")
        .arg("-p")
        .arg("#S")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()?;

    let raw_output = String::from_utf8_lossy(&output.stdout);
    let mut res = raw_output.to_string();

    let len = res.trim_end_matches(&['\r', '\n'][..]).len();

    res.truncate(len);

    Ok(res)
}

fn tmux_is_attached() -> bool {
    env::var_os("TMUX").is_some()
}

fn tmux_list_sessions() -> Result<Vec<String>, Box<dyn Error>> {
    let output = Command::new("tmux")
        .arg("list-session")
        .arg("-F")
        .arg("#S")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()?;

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

fn run_tmux(args: &[&OsStr]) {
    match Command::new("tmux").args(args).spawn().unwrap().wait() {
        Ok(_) => (),
        Err(error) => panic!("help {:?}", error),
    }
}

fn tmux_create_session(name: &str, path: &PathBuf) {
    let tmux_name = tmux_target_name(name);
    run_tmux(&[
        OsStr::new("new-session"),
        OsStr::new("-ds"),
        OsStr::new(&tmux_name),
        OsStr::new("-c"),
        path.as_os_str(),
    ]);
}

fn tmux_swith_session(name: &str) {
    let tmux_name = tmux_target_name(name);
    run_tmux(&[OsStr::new("switch"), OsStr::new("-t"), OsStr::new(&tmux_name)]);
}

fn tmux_attach_session(name: &str) {
    let tmux_name = tmux_target_name(name);
    run_tmux(&[OsStr::new("attach"), OsStr::new("-t"), OsStr::new(&tmux_name)]);
}

fn options_from_path(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|r| {
            format!(
                "{}/{}",
                r.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                r.file_name().unwrap().to_str().unwrap().to_owned()
            )
        })
        .collect()
}

fn display_options_from_options(
    options: Vec<String>,
    live_sessions: &[String],
    attach_session_name: &String,
) -> Vec<String> {
    options
        .into_iter()
        .map(|r| {
            let target = tmux_target_name(&r);
            if attach_session_name == &target {
                return format!("[33m{t}[0m", t = r);
            } else if live_sessions.contains(&target) {
                return format!("[32m{t}[0m", t = r);
            } else {
                return r;
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

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let config = load_config();

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

    let project_paths = get_project_directories(projects)?;
    let project_dir_paths = get_directories(directories)?;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let paths: Vec<PathBuf> = project_paths
        .into_iter()
        .chain(project_dir_paths)
        .filter(|p| seen.insert(p.clone()))
        .collect();

    let options = options_from_path(paths.clone());
    let mut order: Vec<usize> = (0..options.len()).collect();
    order.sort_by_key(|&i| options[i].to_lowercase());
    let paths: Vec<PathBuf> = order.iter().map(|&i| paths[i].clone()).collect();
    let options: Vec<String> = order.iter().map(|&i| options[i].clone()).collect();

    let live_sessions = tmux_list_sessions()?;
    let attach_session_name = tmux_attached_session_name()?;
    let display_options =
        display_options_from_options(options.clone(), &live_sessions, &attach_session_name);
    let selection = match select_with_tv(display_options.clone()) {
        Some(s) => s,
        None => {
            println!("no selection made");
            std::process::exit(1);
        }
    };

    let strip_ansi = |s: &str| -> String {
        let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        re.replace_all(s, "").to_string()
    };
    let clean_selection = strip_ansi(&selection);

    let project_path;
    let project_name;

    match options.iter().position(|r| r.eq(&clean_selection)) {
        Some(index) => {
            project_path = &paths[index];
            project_name = &options[index];
        }
        _none => {
            println!("no index found for selected option");
            std::process::exit(1)
        }
    };

    let is_attached = tmux_is_attached();

    if !live_sessions.contains(&tmux_target_name(project_name)) {
        tmux_create_session(project_name, project_path);
    }

    println!("{}", is_attached);
    if is_attached {
        tmux_swith_session(project_name);
    } else {
        tmux_attach_session(project_name);
    }

    Ok(())
}
