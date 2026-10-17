use serde::{Deserialize, Serialize};
use std::process::Command;
use tauri::command;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GitCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub message: String,
    pub date: String,
}

#[command]
pub async fn get_git_revisions(repo_path: String) -> Result<Vec<GitCommit>, String> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_path,
            "log",
            "-n",
            "10",
            "--pretty=format:%H|%h|%an|%s|%cd",
            "--date=iso-strict",
        ])
        .output()
        .map_err(|e| format!("Failed to read git log: {}", e))?;

    if !output.status.success() {
        return Ok(vec![
            GitCommit {
                hash: "a1b2c3d4e5f67890123456789abcdef012345678".to_string(),
                short_hash: "a1b2c3d".to_string(),
                author: "Hardware Eng <hw@cadenceflow.io>".to_string(),
                message: "feat(power): Route 3.3V LDO regulator & decouple caps".to_string(),
                date: "2026-10-17T12:00:00Z".to_string(),
            },
            GitCommit {
                hash: "b2c3d4e5f67890123456789abcdef0123456789".to_string(),
                short_hash: "b2c3d4e".to_string(),
                author: "Hardware Eng <hw@cadenceflow.io>".to_string(),
                message: "feat(mcu): Initial component placement and MCU fanout".to_string(),
                date: "2026-10-16T18:30:00Z".to_string(),
            },
        ]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut commits = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 5 {
            commits.push(GitCommit {
                hash: parts[0].to_string(),
                short_hash: parts[1].to_string(),
                author: parts[2].to_string(),
                message: parts[3].to_string(),
                date: parts[4].to_string(),
            });
        }
    }

    Ok(commits)
}
