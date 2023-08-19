use clap::Parser;
use rust_fzf::select;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(author = "Liam Ederzeel", about = "", long_about = None)]
struct Cli {
    #[arg(short = 'd')]
    directories: Vec<String>,
}

fn get_directories(directories: Vec<String>) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths: Vec<Vec<PathBuf>> = vec![];

    println!("{:?}", directories);

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

fn list_tmux_sessions() -> Result<Vec<String>, Box<dyn Error>> {
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
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<String>>();

    Ok(res)
}

fn options_from_path(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|r| r.file_name().unwrap().to_str().unwrap().to_owned())
        .collect()
}

fn display_options_from_options(options: Vec<String>, live_sessions: &Vec<String>) -> Vec<String> {
    options
        .into_iter()
        .map(|r| {
            if live_sessions.contains(&r) {
                return format!("[34m{t}[0m", t = r);
            } else {
                return r;
            }
        })
        .collect()
}

fn main() {
    let cli = Cli::parse();

    let directories = cli.directories;

    let paths = match get_directories(directories) {
        Ok(paths) => paths,
        Err(error) => panic!("help {}", error),
    };
    let options = options_from_path(paths.clone());
    let live_sessions = match list_tmux_sessions() {
        Ok(list) => list,
        Err(error) => panic!("help {}", error),
    };
    let display_options = display_options_from_options(options.clone(), &live_sessions);
    let selection = select(display_options.clone(), vec!["--ansi".to_string()]);

    let project_path;
    let project_name;

    match options.iter().position(|r| r.eq(&selection)) {
        Some(index) => {
            project_path = &paths[index];
            project_name = &options[index];
        }
        None => {
            println!("no index found for selected option");
            std::process::exit(1)
        }
    };

    if live_sessions.contains(&selection) {
        match Command::new("tmux")
            .arg("switch")
            .arg("-t")
            .arg("backend")
            .spawn()
            .unwrap()
            .wait()
        {
            Ok(_) => (),
            Err(error) => panic!("help {:?}", error),
        }
    } else {
        match Command::new("tmux")
            .arg("new-session")
            .arg("-ds")
            .arg(&project_name)
            .arg("-c")
            .arg(&project_path)
            .spawn()
            .unwrap()
            .wait()
        {
            Ok(_) => (),
            Err(error) => panic!("help {:?}", error),
        }
        match Command::new("tmux")
            .arg("switch")
            .arg("-t")
            .arg(&project_name)
            .spawn()
            .unwrap()
            .wait()
        {
            Ok(_) => (),
            Err(error) => panic!("help {:?}", error),
        }
    }
}
