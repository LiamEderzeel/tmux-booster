use clap::Parser;
use rust_fzf::select;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

#[derive(Parser)]
#[command(author, about, long_about = None)]
struct Cli {
    #[arg(
        short = 'd',
        help = "path or paths to directory containing project directories."
    )]
    project_directories: Vec<String>,

    #[arg(
        short = 'p',
        help = "path or paths to project directory."
    )]
    projects: Vec<String>
}

fn get_project_directories(directories: Vec<String>) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths: Vec<PathBuf> = vec![];

    for directory in &directories {
        paths.push(PathBuf::from(directory));
    }
Ok(paths)
}

fn get_directories(directories: Vec<String>) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths: Vec<Vec<PathBuf>> = vec![];

    for directory in &directories {
        let res = fs::read_dir(Path::new(directory))?;

        paths.push(
            res.into_iter()
                .filter(|r| r.is_ok()) // Get rid of Err variants for Result<DirEntry>
                .map(|r| r.unwrap().path()) // This is safe, since we only have the Ok variants
                .filter(|r| r.is_dir()) // Filter out non-folders
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
        .split("\n")
        .map(str::to_string)
        .collect::<Vec<String>>();

    Ok(res)
}

fn tmux_create_session(name: &String, path: &PathBuf) {
    match Command::new("tmux")
        .arg("new-session")
        .arg("-ds")
        .arg(name)
        .arg("-c")
        .arg(path)
        .spawn()
        .unwrap()
        .wait()
    {
        Ok(_) => (),
        Err(error) => panic!("help {:?}", error),
    }
}

fn tmux_swith_session(name: &str) {
    let tmux_name = name.replace('.', "_");
    match Command::new("tmux")
        .arg("switch")
        .arg("-t")
        .arg(&tmux_name)
        .spawn()
        .unwrap()
        .wait()
    {
        Ok(_) => (),
        Err(error) => panic!("help {:?}", error),
    }
}

fn tmux_attach_session(name: &str) {
    let tmux_name = name.replace('.', "_");
    match Command::new("tmux")
        .arg("attach")
        .arg("-t")
        .arg(&tmux_name)
        .spawn()
        .unwrap()
        .wait()
    {
        Ok(_) => (),
        Err(error) => panic!("help {:?}", error),
    }
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
            if attach_session_name == &r {
                return format!("[33m{t}[0m", t = r);
            } else if live_sessions.contains(&r) {
                return format!("[34m{t}[0m", t = r);
            } else {
                return r;
            }
        })
        .collect()
}

fn main() {
    let cli = Cli::parse();

    let directories = cli.project_directories;
    let projects = cli.projects;

    let project_paths = match get_project_directories(projects) {
        Ok(paths) => paths,
        Err(error) => panic!("help {}", error),
    };

    let project_dir_paths = match get_directories(directories) {
        Ok(paths) => paths,
        Err(error) => panic!("help {}", error),
    };

    let paths: Vec<PathBuf> = project_paths.into_iter().chain(project_dir_paths).collect();

    let options = options_from_path(paths.clone());

    let live_sessions = match tmux_list_sessions() {
        Ok(list) => list,
        Err(error) => panic!("help {}", error),
    };
    let attach_session_name = match tmux_attached_session_name() {
        Ok(attach_session_name) => attach_session_name,
        Err(error) => panic!("help {}", error),
    };
    let display_options =
        display_options_from_options(options.clone(), &live_sessions, &attach_session_name);
    let selection = select(display_options.clone(), vec!["--ansi".to_string()]);

    let project_path;
    let project_name;

    match options.iter().position(|r| r.eq(&selection)) {
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

    if !live_sessions.contains(&selection) {
        tmux_create_session(project_name, project_path);
    }

    println!("{}", is_attached);
    if is_attached {
        tmux_swith_session(project_name);
    } else {
        tmux_attach_session(project_name);
    }
}
