use super::FileFormatter;
use crate::error::Result;
use crate::plugin::PluginManager;
use crate::utils::color::{colorize_file_name, colorize_file_name_with_icon};
use crate::utils::icons::format_with_icon;
use colored::*;
use console::strip_ansi_codes;
use lla_plugin_interface::proto::DecoratedEntry;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use unicode_width::UnicodeWidthStr;

pub struct GitFormatter {
    pub show_icons: bool,
}

impl GitFormatter {
    pub fn new(show_icons: bool) -> Self {
        Self { show_icons }
    }

    fn strip_ansi(s: &str) -> String {
        String::from_utf8(strip_ansi_escapes::strip(s).unwrap_or_default()).unwrap_or_default()
    }
}
#[derive(Debug, Default)]
struct GitInfo {
    branch: String,
    ahead: usize,
    behind: usize,
}

impl GitFormatter {
    fn get_git_info(path: &Path) -> Option<GitInfo> {
        let status = Command::new("git")
            .arg("rev-parse")
            .arg("--is-inside-work-tree")
            .current_dir(path)
            .output()
            .ok()?;

        if !status.status.success() {
            return None;
        }

        let mut info = GitInfo::default();

        if let Ok(output) = Command::new("git")
            .args(["status", "-b", "--porcelain=v2"])
            .current_dir(path)
            .output()
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if line.starts_with("# branch.head ") {
                    info.branch = line.split_whitespace().nth(2)?.to_string();
                } else if line.starts_with("# branch.ab ") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        info.ahead = parts[2].trim_start_matches('+').parse().unwrap_or(0);
                        info.behind = parts[3].trim_start_matches('-').parse().unwrap_or(0);
                    }
                }
            }
        }

        Some(info)
    }

    fn format_git_status(status: &str) -> (String, String) {
        let status_str = match status {
            "M." => "[staged]".green(),
            "A." => "[added]".green(),
            "D." => "[deleted]".red(),
            "R." => "[renamed]".blue(),
            "C." => "[copied]".blue(),

            ".M" => "[modified]".yellow(),
            ".D" => "[deleted]".red(),

            "MM" => "[modified*]".yellow(),
            "AM" => "[added+]".green(),
            "DM" => "[deleted*]".red(),

            "UU" => "[conflict]".red(),

            "??" => "[untracked]".bright_black(),
            "!!" => "[ignored]".bright_black(),
            "." => "[unchanged]".normal(),

            s if s.starts_with('M') => format!("[modified+{}]", &s[1..]).yellow(),
            s if s.starts_with('A') => format!("[added+{}]", &s[1..]).green(),
            s if s.starts_with('D') => format!("[deleted+{}]", &s[1..]).red(),
            s if s.starts_with('R') => format!("[renamed+{}]", &s[1..]).blue(),
            s if s.starts_with('C') => format!("[copied+{}]", &s[1..]).blue(),
            s => format!("[{}]", s).normal(),
        };

        (status_str.to_string(), "".to_string())
    }

    fn get_git_status_map(workspace_root: &Path) -> HashMap<String, String> {
        let mut status_map = HashMap::new();

        if let Ok(output) = Command::new("git")
            .args(["status", "--porcelain=v2", "--untracked-files=all"])
            .current_dir(workspace_root)
            .output()
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() {
                    continue;
                }

                match parts[0] {
                    "1" | "2" if parts.len() >= 9 => {
                        let xy = parts[1];
                        let path = parts[8];
                        status_map.insert(path.to_string(), xy.to_string());
                    }
                    "?" if parts.len() >= 2 => {
                        status_map.insert(parts[1].to_string(), "??".to_string());
                    }
                    "!" if parts.len() >= 2 => {
                        status_map.insert(parts[1].to_string(), "!!".to_string());
                    }
                    _ => {}
                }
            }
        }

        if let Ok(output) = Command::new("git")
            .args(["ls-files"])
            .current_dir(workspace_root)
            .output()
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                status_map
                    .entry(line.to_string())
                    .or_insert_with(|| ".".to_string());
            }
        }

        status_map
    }

    fn get_last_commit_info(path: &Path, file_path: &Path) -> Option<(String, String, String)> {
        let output = Command::new("git")
            .args([
                "log",
                "-1",
                "--format=%h|%s|%cr|%an",
                "--",
                file_path.to_str()?,
            ])
            .current_dir(path)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let log = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = log.trim().split('|').collect();
        if parts.len() >= 4 {
            Some((
                parts[0].to_string(),
                parts[2].to_string(),
                parts[3].to_string(),
            ))
        } else {
            None
        }
    }
}

impl FileFormatter for GitFormatter {
    fn format_files(
        &self,
        files: &[DecoratedEntry],
        plugin_manager: &mut PluginManager,
        _depth: Option<usize>,
    ) -> Result<String> {
        if files.is_empty() {
            return Ok(String::new());
        }

        let workspace_root = Path::new(&files[0].path)
            .ancestors()
            .find(|p| p.join(".git").exists())
            .unwrap_or(Path::new("."));

        let git_info = match GitFormatter::get_git_info(workspace_root) {
            Some(info) => info,
            None => return Ok("Not a git repository".red().to_string()),
        };

        let status_map = GitFormatter::get_git_status_map(workspace_root);
        let mut max_name_width: usize = 0;
        let mut max_hash_width: usize = 0;
        let mut max_time_width: usize = 0;
        let mut max_author_width: usize = 0;

        for file in files {
            let path = Path::new(&file.path);
            let name = colorize_file_name(path);
            let name_with_icon = colorize_file_name_with_icon(
                path,
                format_with_icon(path, name.to_string(), self.show_icons),
            );
            let commit_info = GitFormatter::get_last_commit_info(workspace_root, path)
                .unwrap_or_else(|| ("-".to_string(), "never".to_string(), "-".to_string()));

            max_name_width = max_name_width.max(Self::strip_ansi(&name_with_icon).width());
            max_hash_width = max_hash_width.max(commit_info.0.len());
            max_time_width = max_time_width.max(commit_info.1.len());
            max_author_width = max_author_width.max(commit_info.2.len());
        }

        let mut output = format!(
            "\n{} {}{}{}\n{}\n",
            "⎇".bright_blue(),
            git_info.branch.green().bold(),
            if git_info.ahead > 0 {
                format!(" ↑{}", git_info.ahead).yellow()
            } else {
                "".into()
            },
            if git_info.behind > 0 {
                format!(" ↓{}", git_info.behind).red()
            } else {
                "".into()
            },
            "─".repeat(40).bright_black()
        );

        output.push_str(&format!(
            "{}  {}  {}  {}  {}\n",
            "Name".bold(),
            "Commit".bold(),
            "Time".bold(),
            "Author".bold(),
            "Status".bold()
        ));
        output.push_str(
            &"─"
                .repeat(max_name_width + max_hash_width + max_time_width + max_author_width + 40)
                .bright_black(),
        );
        output.push_str("\n");

        let mut entries = Vec::new();
        for file in files {
            let path = Path::new(&file.path);
            let name = colorize_file_name(path);
            let name_with_icon = colorize_file_name_with_icon(
                path,
                format_with_icon(path, name.to_string(), self.show_icons),
            );

            let relative_path = path.strip_prefix(workspace_root).unwrap_or(path);
            let relative_path_str = relative_path.to_string_lossy();

            let (status_str, _) = status_map
                .get(relative_path_str.as_ref())
                .map(|s| GitFormatter::format_git_status(s))
                .unwrap_or_else(|| ("".to_string(), "".to_string()));

            let commit_info = GitFormatter::get_last_commit_info(workspace_root, relative_path)
                .unwrap_or_else(|| ("-".to_string(), "never".to_string(), "-".to_string()));

            let plugin_fields = plugin_manager.format_fields(file, "git").join(" ");

            entries.push((name_with_icon, commit_info, status_str, plugin_fields));
        }

        for (name, commit_info, status, plugin_fields) in entries {
            let name_width = strip_ansi_codes(&name).width();
            let name_padding = " ".repeat(max_name_width.saturating_sub(name_width) as usize);

            let hash_padding =
                " ".repeat(max_hash_width.saturating_sub(commit_info.0.len()) as usize);
            let time_padding =
                " ".repeat(max_time_width.saturating_sub(commit_info.1.len()) as usize);

            let author_part = if commit_info.2 != "-" {
                format!("by {} ", commit_info.2.bright_blue())
            } else {
                "".into()
            };

            output.push_str(&format!(
                "{}{}  @{}{}  {}{}  {}{}{}",
                name,
                name_padding,
                commit_info.0.bright_yellow(),
                hash_padding,
                commit_info.1.bright_black(),
                time_padding,
                author_part,
                status,
                if plugin_fields.is_empty() {
                    String::new()
                } else {
                    format!(" {}", plugin_fields)
                }
            ));
            output.push('\n');
        }

        Ok(output)
    }
}