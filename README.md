# Tmux Booster

# Configuration

tmux-booster can be configured via a TOML file located at:

```
~/.config/tmux-booster/config.toml
```

If the file does not exist, tmux-booster will run with defaults and rely solely on command-line arguments.

---

## Options

### `directory_paths`

A list of directories that contain project directories. tmux-booster will scan each entry and treat its subdirectories as projects.

```toml
directory_paths = [
    "~/projects",
    "~/work",
]
```

### `project_paths`

A list of direct paths to individual project directories.

```toml
project_paths = [
    "~/projects/my-app",
    "~/dotfiles",
]
```

---

## Full example

```toml
directory_paths = [
    "~/projects",
    "~/work",
    "~/clients",
]

project_paths = [
    "~/dotfiles",
    "~/projects/special-repo",
]
```

---

## Command-line arguments

Both lists can also be provided or extended via CLI flags:

| Flag | Description                                        |
| ---- | -------------------------------------------------- |
| `-d` | Path to a directory containing project directories |
| `-p` | Path to a direct project directory                 |

Flags can be repeated to pass multiple values:

```bash
tmux-booster -d ~/projects -d ~/work -p ~/dotfiles
```

### Merging config and CLI

If both a config file and CLI arguments are provided, they are merged. The config file acts as a persistent baseline and CLI arguments are appended on top. Duplicates are automatically removed.

---

## Notes

- Paths support `~` as a shorthand for your home directory.
- Non-existent paths will produce an error at runtime.
- The config file is optional, tmux-booster works without it.
