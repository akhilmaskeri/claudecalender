use anyhow::{Context, Result};
use chrono::{Datelike, Local, Months, NaiveDate};
use crate::models::UsageData;

pub struct BillingPeriod {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

pub fn fetch_account(session_key: &str) -> Result<serde_json::Value> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .get("https://claude.ai/api/account")
        .header("Cookie", format!("sessionKey={}", session_key))
        .header("Accept", "application/json")
        .send()
        .context("failed to call claude.ai/api/account")?;

    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .context("failed to parse API response")?;

    if status == 401 || status == 403 {
        anyhow::bail!(
            "session rejected ({}). Run again to re-enter your sessionKey.\n\
             Hint: delete ~/.config/claudecalender/session.json to reset.",
            status
        );
    }

    Ok(body)
}

pub fn fetch_org_uuid(session_key: &str) -> Result<String> {
    let client = reqwest::blocking::Client::new();
    let body: serde_json::Value = client
        .get("https://claude.ai/api/organizations")
        .header("Cookie", format!("sessionKey={}", session_key))
        .header("Accept", "application/json")
        .send()
        .context("failed to call organizations")?
        .json()
        .context("failed to parse organizations response")?;
    body[0]["uuid"]
        .as_str()
        .context("org uuid not found")
        .map(|s| s.to_string())
}

pub fn fetch_usage(session_key: &str, org_uuid: &str) -> Result<UsageData> {
    let client = reqwest::blocking::Client::new();
    let url = format!("https://claude.ai/api/organizations/{}/usage", org_uuid);
    let body: serde_json::Value = client
        .get(&url)
        .header("Cookie", format!("sessionKey={}", session_key))
        .header("Accept", "application/json")
        .send()
        .context("failed to call usage endpoint")?
        .json()
        .context("failed to parse usage response")?;
    Ok(UsageData {
        five_hour_pct: body["five_hour"]["utilization"].as_f64().unwrap_or(0.0),
        five_hour_resets_at: body["five_hour"]["resets_at"].as_str().unwrap_or("").to_string(),
        seven_day_pct: body["seven_day"]["utilization"].as_f64().unwrap_or(0.0),
        seven_day_resets_at: body["seven_day"]["resets_at"].as_str().unwrap_or("").to_string(),
    })
}

pub fn billing_period(body: &serde_json::Value) -> Result<BillingPeriod> {
    // Use the account creation date to determine billing day-of-month.
    // Stripe subscriptions bill on the same day each month as the original signup.
    let created_at = body
        .get("created_at")
        .and_then(|v| v.as_str())
        .context("created_at not found in account response")?;

    let created = NaiveDate::parse_from_str(&created_at[..10], "%Y-%m-%d")
        .with_context(|| format!("unexpected created_at format: {created_at}"))?;

    let billing_day = created.day();
    let today = Local::now().date_naive();

    let start = if today.day() >= billing_day {
        NaiveDate::from_ymd_opt(today.year(), today.month(), billing_day)
            .context("invalid billing start date")?
    } else {
        let last_month = today
            .checked_sub_months(Months::new(1))
            .context("date underflow")?;
        NaiveDate::from_ymd_opt(last_month.year(), last_month.month(), billing_day)
            .context("invalid billing start date")?
    };

    let end = start
        .checked_add_months(Months::new(1))
        .context("date overflow calculating period end")?;

    Ok(BillingPeriod { start, end })
}
