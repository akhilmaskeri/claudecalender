use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAiOauth {
    pub subscription_type: String,
    #[allow(dead_code)]
    pub access_token: String,
    #[allow(dead_code)]
    pub refresh_token: String,
    #[allow(dead_code)]
    pub expires_at: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub claude_ai_oauth: ClaudeAiOauth,
}

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub session_key: String,
}

#[derive(Serialize)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_efficiency_pct: f64,
    pub cost_usd: f64,
}

#[derive(Serialize)]
pub struct SessionStats {
    pub session_id: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_mins: f64,
    pub project: String,
    /// Working directory the session was launched from, read from the `cwd`
    /// field in the JSONL. `None` for stub sessions that never recorded a turn.
    pub cwd: Option<String>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub peak_context_tokens: u64,
    pub peak_context_pct: f64,
    pub cost_usd: f64,
    pub models: Vec<ModelUsage>,
    pub summary: Option<String>,
}

pub struct UsageData {
    pub five_hour_pct: f64,
    pub five_hour_resets_at: String,
    pub seven_day_pct: f64,
    pub seven_day_resets_at: String,
}

#[derive(Serialize)]
pub struct Output {
    pub plan: String,
    pub subscription_start_date: String,
    pub subscription_end_date: String,
    pub sessions: Vec<SessionStats>,
    #[serde(skip)]
    pub org_uuid: String,
    #[serde(skip)]
    pub session_key: String,
    #[serde(skip)]
    pub initial_usage: Option<UsageData>,
}
