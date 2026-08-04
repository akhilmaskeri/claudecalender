use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use crate::models::{ModelUsage, SessionStats};

pub fn scan_sessions(since: &NaiveDate) -> Result<Vec<SessionStats>> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    let projects_dir = home.join(".claude").join("projects");

    if !projects_dir.exists() {
        return Ok(vec![]);
    }

    let mut sessions = Vec::new();

    for project_entry in fs::read_dir(&projects_dir)? {
        let project_entry = project_entry?;
        if !project_entry.file_type()?.is_dir() {
            continue;
        }

        let project_path = decode_project_dir(&project_entry.file_name().to_string_lossy());

        for file_entry in fs::read_dir(project_entry.path())? {
            let file_entry = file_entry?;
            let fname = file_entry.file_name();
            let fname = fname.to_string_lossy();

            if !fname.ends_with(".jsonl") || file_entry.file_type()?.is_dir() {
                continue;
            }

            let session_id = fname.trim_end_matches(".jsonl").to_string();

            match parse_session(file_entry.path(), since, session_id, project_path.clone()) {
                Ok(Some(stats)) => sessions.push(stats),
                Ok(None) => {}
                Err(e) => eprintln!("Warning: skipping {:?}: {e}", file_entry.path()),
            }
        }
    }

    sessions.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    Ok(sessions)
}

fn parse_session(
    path: std::path::PathBuf,
    since: &NaiveDate,
    session_id: String,
    project: String,
) -> Result<Option<SessionStats>> {
    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Scan lines until we find one with a top-level "timestamp" field.
    // Early lines like "permission-mode" and "file-history-snapshot" don't have it.
    let started_at = loop {
        match lines.next() {
            None => return Ok(None),
            Some(Err(e)) => return Err(e.into()),
            Some(Ok(line)) if line.trim().is_empty() => continue,
            Some(Ok(line)) => {
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let ts = match v.get("timestamp").and_then(|t| t.as_str()) {
                    Some(ts) if ts.len() >= 10 => ts.to_string(),
                    _ => continue,
                };
                let date = NaiveDate::parse_from_str(&ts[..10], "%Y-%m-%d")?;
                if date < *since {
                    return Ok(None);
                }
                break ts;
            }
        }
    };

    // Accumulate token usage per model; also track last timestamp and peak context.
    let mut model_map: HashMap<String, (u64, u64, u64, u64)> = HashMap::new();
    let mut last_ts = started_at.clone();
    let mut peak_context: u64 = 0;
    let mut peak_context_model = String::from("unknown");
    let mut summary: Option<String> = None;

    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str())
            && ts.len() >= 10
        {
            last_ts = ts.to_string();
        }

        if v.get("type").and_then(|t| t.as_str()) == Some("ai-title")
            && let Some(s) = v.get("aiTitle").and_then(|s| s.as_str())
        {
            summary = Some(s.to_string());
        }

        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }

        let usage = match v.pointer("/message/usage") {
            Some(u) => u,
            None => continue,
        };
        let model = v.pointer("/message/model")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
            .to_string();

        let input   = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let output  = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let created = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let read    = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

        let ctx = input + created + read;
        if ctx > peak_context {
            peak_context = ctx;
            peak_context_model = model.clone();
        }

        let entry = model_map.entry(model).or_insert((0, 0, 0, 0));
        entry.0 += input;
        entry.1 += output;
        entry.2 += created;
        entry.3 += read;
    }

    if model_map.is_empty() {
        return Ok(None);
    }

    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cost = 0f64;
    let mut models: Vec<ModelUsage> = model_map
        .into_iter()
        .map(|(model, (input, output, created, read))| {
            total_input += input;
            total_output += output;
            let cache_total = created + read;
            let cache_efficiency_pct = if cache_total > 0 {
                (read as f64 / cache_total as f64) * 100.0
            } else {
                0.0
            };
            let (ip, op, cwp, crp) = model_price(&model);
            let cost_usd = (input as f64 * ip
                + output as f64 * op
                + created as f64 * cwp
                + read as f64 * crp)
                / 1_000_000.0;
            total_cost += cost_usd;
            ModelUsage {
                model,
                input_tokens: input,
                output_tokens: output,
                cache_creation_tokens: created,
                cache_read_tokens: read,
                cache_efficiency_pct: (cache_efficiency_pct * 10.0).round() / 10.0,
                cost_usd,
            }
        })
        .collect();

    models.sort_by_key(|m| std::cmp::Reverse(m.input_tokens));

    let duration_mins = parse_duration_mins(&started_at, &last_ts);
    let context_limit = model_context_limit(&peak_context_model);
    let peak_context_pct = (peak_context as f64 / context_limit as f64 * 1000.0).round() / 10.0;

    Ok(Some(SessionStats {
        session_id,
        started_at,
        ended_at: last_ts,
        duration_mins,
        project,
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        peak_context_tokens: peak_context,
        peak_context_pct,
        cost_usd: total_cost,
        models,
        summary,
    }))
}

// Returns (input, output, cache_write, cache_read) prices per million tokens.
// Pricing as of May 2026: Opus 4.7 $5/$25, Sonnet 4.6 $3/$15, Haiku 4.5 $1/$5.
// Cache writes cost 1.25× base input; cache reads cost 0.1× base input.
fn model_price(model: &str) -> (f64, f64, f64, f64) {
    if model.contains("opus") {
        (5.0, 25.0, 6.25, 0.50)
    } else if model.contains("haiku") {
        (1.0, 5.0, 1.25, 0.10)
    } else {
        (3.0, 15.0, 3.75, 0.30)
    }
}

fn model_context_limit(model: &str) -> u64 {
    // All current Claude models (Opus/Sonnet/Haiku 4.x) have a 200k context window.
    // Older claude-2 / claude-instant models had 100k.
    if model.contains("claude-2") || model.contains("instant") {
        100_000
    } else {
        200_000
    }
}

fn parse_duration_mins(start: &str, end: &str) -> f64 {
    let parse = |s: &str| DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc));
    match (parse(start), parse(end)) {
        (Ok(s), Ok(e)) => {
            let secs = (e - s).num_seconds();
            if secs < 0 { 0.0 } else { (secs as f64 / 60.0 * 10.0).round() / 10.0 }
        }
        _ => 0.0,
    }
}

fn decode_project_dir(encoded: &str) -> String {
    // ~/.claude/projects encodes paths by replacing '/' with '-'
    // e.g. "-home-akhil-code-myproject" → "/home/akhil/code/myproject"
    let s = encoded.replace('-', "/");
    // The leading char was '/' encoded as '-', so result starts with '/'
    if s.starts_with('/') { s } else { format!("/{}", s) }
}
