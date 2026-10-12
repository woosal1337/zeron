//! Wire types for the signed-in GitHub activity panel.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitHubActivityPeriod {
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivityStat {
    pub count: u32,
    pub delta: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivityDay {
    pub date: String,
    pub count: u32,
    pub level: u8,
    pub weekday: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivityWeek {
    pub days: Vec<GitHubActivityDay>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivityPeak {
    pub label: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivitySnapshot {
    pub period: GitHubActivityPeriod,
    pub repository: Option<String>,
    pub repositories: Vec<String>,
    pub contributions: GitHubActivityStat,
    pub pull_requests_opened: GitHubActivityStat,
    pub pull_requests_merged: GitHubActivityStat,
    pub reviews: GitHubActivityStat,
    pub year_total: u32,
    pub weeks: Vec<GitHubActivityWeek>,
    pub current_streak_days: u32,
    pub longest_streak_days: u32,
    pub most_active_month: Option<GitHubActivityPeak>,
    pub most_active_weekday: Option<GitHubActivityPeak>,
    pub attention_count: u32,
    pub attention_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_serializes_as_lower_camel_case() {
        assert_eq!(
            serde_json::to_value(GitHubActivityPeriod::Week).unwrap(),
            serde_json::json!("week")
        );
        assert_eq!(
            serde_json::to_value(GitHubActivityPeriod::Month).unwrap(),
            serde_json::json!("month")
        );
        assert_eq!(
            serde_json::to_value(GitHubActivityPeriod::Year).unwrap(),
            serde_json::json!("year")
        );
    }
}
