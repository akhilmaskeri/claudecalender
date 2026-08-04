use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Write};
use crate::models::{Credentials, Session};

pub fn read_plan() -> Result<String> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    let path = home.join(".claude").join(".credentials.json");
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let creds: Credentials = serde_json::from_str(&raw)
        .context("failed to parse .credentials.json")?;
    Ok(creds.claude_ai_oauth.subscription_type)
}

pub fn load_session() -> Result<String> {
    let session_path = session_file_path()?;

    if session_path.exists() {
        let raw = fs::read_to_string(&session_path)
            .context("failed to read saved session")?;
        let session: Session = serde_json::from_str(&raw)
            .context("failed to parse saved session")?;
        return Ok(session.session_key);
    }

    prompt_and_save_session()
}

pub fn prompt_and_save_session() -> Result<String> {
    eprintln!();
    eprintln!("No session found. To authenticate:");
    eprintln!("  1. Open https://claude.ai in your browser and log in");
    eprintln!("  2. Open DevTools (F12) → Application → Cookies → https://claude.ai");
    eprintln!("  3. Copy the value of the 'sessionKey' cookie");
    eprintln!();
    eprint!("Paste sessionKey here: ");
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let key = input.trim().to_string();
    anyhow::ensure!(!key.is_empty(), "session key cannot be empty");

    let session_path = session_file_path()?;
    if let Some(parent) = session_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let session = Session { session_key: key.clone() };
    fs::write(&session_path, serde_json::to_string(&session)?)
        .with_context(|| format!("failed to save session to {}", session_path.display()))?;

    eprintln!("Session saved to {}", session_path.display());
    Ok(key)
}

fn session_file_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(".config").join("claudecalender").join("session.json"))
}
