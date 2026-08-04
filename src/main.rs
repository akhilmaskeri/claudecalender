mod api;
mod auth;
mod models;
mod sessions;
mod ui;

use anyhow::Result;
use models::Output;

fn main() -> Result<()> {
    let plan = auth::read_plan()?;
    let session_key = auth::load_session()?;

    let body = api::fetch_account(&session_key).or_else(|e| {
        eprintln!("Warning: {e}");
        let new_key = auth::prompt_and_save_session()?;
        api::fetch_account(&new_key)
    })?;

    let period = api::billing_period(&body)?;
    let session_stats = sessions::scan_sessions(&period.start)?;

    let org_uuid = api::fetch_org_uuid(&session_key).unwrap_or_default();
    let initial_usage = api::fetch_usage(&session_key, &org_uuid).ok();

    let output = Output {
        plan,
        subscription_start_date: period.start.to_string(),
        subscription_end_date: period.end.to_string(),
        sessions: session_stats,
        org_uuid,
        session_key,
        initial_usage,
    };

    ui::run_app(output)
}
