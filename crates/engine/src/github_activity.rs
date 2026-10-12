//! Period windows, GitHub payload parsing, and streak rules.
use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, Utc};
use serde_json::Value;
use zeron_proto::{
    GitHubActivityDay, GitHubActivityPeak, GitHubActivityPeriod, GitHubActivitySnapshot,
    GitHubActivityStat, GitHubActivityWeek,
};

pub(crate) const ACTIVITY_QUERY: &str = "query($from:DateTime!,$to:DateTime!,$prevFrom:DateTime!,$prevTo:DateTime!,$yearFrom:DateTime!,$yearTo:DateTime!,$opened:String!,$openedPrev:String!,$merged:String!,$mergedPrev:String!,$attention:String!){viewer{contributionsCollection(from:$from,to:$to){contributionCalendar{totalContributions}totalPullRequestReviewContributionscommitContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}issueContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}pullRequestContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}pullRequestReviewContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}}previous:contributionsCollection(from:$prevFrom,to:$prevTo){contributionCalendar{totalContributions}totalPullRequestReviewContributionscommitContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}issueContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}pullRequestContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}pullRequestReviewContributionsByRepository(maxRepositories:50){contributions{totalCount}repository{nameWithOwner}}}year:contributionsCollection(from:$yearFrom,to:$yearTo){contributionCalendar{totalContributions weeks{contributionDays{contributionCount contributionLevel date weekday}}}commitContributionsByRepository(maxRepositories:50){repository{nameWithOwner}}issueContributionsByRepository(maxRepositories:50){repository{nameWithOwner}}pullRequestContributionsByRepository(maxRepositories:50){repository{nameWithOwner}}pullRequestReviewContributionsByRepository(maxRepositories:50){repository{nameWithOwner}}}}opened:search(query:$opened,type:ISSUE,first:1){issueCount}openedPrev:search(query:$openedPrev,type:ISSUE,first:1){issueCount}merged:search(query:$merged,type:ISSUE,first:1){issueCount}mergedPrev:search(query:$mergedPrev,type:ISSUE,first:1){issueCount}attention:search(query:$attention,type:ISSUE,first:1){issueCount}}";

const CONTRIBUTION_KEYS: [&str; 4] = [
    "commitContributionsByRepository",
    "issueContributionsByRepository",
    "pullRequestContributionsByRepository",
    "pullRequestReviewContributionsByRepository",
];

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const WEEKDAYS: [&str; 7] = [
    "Sundays",
    "Mondays",
    "Tuesdays",
    "Wednesdays",
    "Thursdays",
    "Fridays",
    "Saturdays",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivitySpec {
    pub period: GitHubActivityPeriod,
    pub repository: Option<String>,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub previous_from: DateTime<Utc>,
    pub previous_to: DateTime<Utc>,
    pub year_from: DateTime<Utc>,
    pub year_to: DateTime<Utc>,
    pub opened: String,
    pub opened_previous: String,
    pub merged: String,
    pub merged_previous: String,
    pub attention: String,
}

impl ActivitySpec {
    pub(crate) fn variables(&self) -> [(&'static str, String); 11] {
        [
            ("from", stamp(self.from)),
            ("to", stamp(self.to)),
            ("prevFrom", stamp(self.previous_from)),
            ("prevTo", stamp(self.previous_to)),
            ("yearFrom", stamp(self.year_from)),
            ("yearTo", stamp(self.year_to)),
            ("opened", self.opened.clone()),
            ("openedPrev", self.opened_previous.clone()),
            ("merged", self.merged.clone()),
            ("mergedPrev", self.merged_previous.clone()),
            ("attention", self.attention.clone()),
        ]
    }
}

pub(crate) fn repository_name_ok(repository: &str) -> bool {
    let parts: Vec<_> = repository.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
}

pub(crate) fn activity_spec(
    period: GitHubActivityPeriod,
    repository: Option<String>,
    now: DateTime<Utc>,
) -> Result<ActivitySpec, ()> {
    if repository
        .as_deref()
        .is_some_and(|repository| !repository_name_ok(repository))
    {
        return Err(());
    }
    let days = match period {
        GitHubActivityPeriod::Week => 7,
        GitHubActivityPeriod::Month => 30,
        GitHubActivityPeriod::Year => 365,
    };
    let (from, to) = bounds(now, days)?;
    let (previous_from, previous_to) = previous_bounds(from, days)?;
    let (year_from, year_to) = bounds(now, 365)?;
    let repository_ref = repository.as_deref();
    let current_dates = search_dates(from, to);
    let previous_dates = search_dates(previous_from, previous_to);
    Ok(ActivitySpec {
        period,
        opened: scoped(
            format!("author:@me is:pr created:{current_dates}"),
            repository_ref,
        ),
        opened_previous: scoped(
            format!("author:@me is:pr created:{previous_dates}"),
            repository_ref,
        ),
        merged: scoped(
            format!("author:@me is:pr is:merged merged:{current_dates}"),
            repository_ref,
        ),
        merged_previous: scoped(
            format!("author:@me is:pr is:merged merged:{previous_dates}"),
            repository_ref,
        ),
        attention: scoped(
            "review-requested:@me is:pr is:open".to_owned(),
            repository_ref,
        ),
        repository,
        from,
        to,
        previous_from,
        previous_to,
        year_from,
        year_to,
    })
}

pub(crate) fn parse_activity(
    payload: &Value,
    spec: &ActivitySpec,
) -> Result<GitHubActivitySnapshot, ()> {
    let data = payload
        .get("data")
        .filter(|value| value.is_object())
        .ok_or(())?;
    let viewer = data
        .get("viewer")
        .filter(|value| value.is_object())
        .ok_or(())?;
    let current = viewer.get("contributionsCollection").ok_or(())?;
    let previous = viewer.get("previous").ok_or(())?;
    let year = viewer.get("year").ok_or(())?;
    let repository = spec.repository.as_deref();
    let contributions = contribution_count(current, repository)?;
    let previous_contributions = contribution_count(previous, repository)?;
    let reviews = review_count(current, repository)?;
    let previous_reviews = review_count(previous, repository)?;
    let calendar = dig(year, &["contributionCalendar"]).ok_or(())?;
    let weeks = parse_weeks(calendar)?;
    let mut repositories = Vec::new();
    push_names(year, &mut repositories)?;
    normalize_names(&mut repositories);
    let flat: Vec<&GitHubActivityDay> = weeks.iter().flat_map(|week| &week.days).collect();
    let today = today_string(spec)?;
    let counted: Vec<&GitHubActivityDay> = flat
        .iter()
        .copied()
        .filter(|day| day.date.as_str() <= today.as_str())
        .collect();
    let current_streak_days = current_streak(&counted, &today);
    let longest_streak_days = longest_streak(&counted);
    let (most_active_month, most_active_weekday) = peaks(&counted);
    drop(flat);
    Ok(GitHubActivitySnapshot {
        period: spec.period,
        repository: spec.repository.clone(),
        repositories,
        contributions: stat(contributions, previous_contributions),
        pull_requests_opened: stat(
            u32_at(data, &["opened", "issueCount"])?,
            u32_at(data, &["openedPrev", "issueCount"])?,
        ),
        pull_requests_merged: stat(
            u32_at(data, &["merged", "issueCount"])?,
            u32_at(data, &["mergedPrev", "issueCount"])?,
        ),
        reviews: stat(reviews, previous_reviews),
        year_total: u32_at(year, &["contributionCalendar", "totalContributions"])?,
        weeks,
        current_streak_days,
        longest_streak_days,
        most_active_month,
        most_active_weekday,
        attention_count: u32_at(data, &["attention", "issueCount"])?,
        attention_url: attention_url(repository),
    })
}

fn bounds(now: DateTime<Utc>, days: i64) -> Result<(DateTime<Utc>, DateTime<Utc>), ()> {
    let today = now.date_naive();
    Ok((
        midnight(today - Duration::days(days - 1))?,
        midnight(today + Duration::days(1))?,
    ))
}

fn previous_bounds(from: DateTime<Utc>, days: i64) -> Result<(DateTime<Utc>, DateTime<Utc>), ()> {
    Ok((midnight(from.date_naive() - Duration::days(days))?, from))
}

fn midnight(date: NaiveDate) -> Result<DateTime<Utc>, ()> {
    Ok(date.and_hms_opt(0, 0, 0).ok_or(())?.and_utc())
}

fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn search_dates(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    let start = from.date_naive();
    let end = (to - Duration::seconds(1)).date_naive();
    format!("{start}..{end}")
}

fn scoped(query: String, repository: Option<&str>) -> String {
    match repository {
        Some(repository) => format!("{query} repo:{repository}"),
        None => query,
    }
}

fn today_string(spec: &ActivitySpec) -> Result<String, ()> {
    let today = (spec.to - Duration::seconds(1)).date_naive();
    Ok(format!(
        "{:04}-{:02}-{:02}",
        today.year(),
        today.month(),
        today.day()
    ))
}

fn stat(count: u32, previous: u32) -> GitHubActivityStat {
    GitHubActivityStat {
        count,
        delta: delta(count, previous),
    }
}

fn delta(count: u32, previous: u32) -> i32 {
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    let previous = i32::try_from(previous).unwrap_or(i32::MAX);
    count.saturating_sub(previous)
}

fn dig<'a>(mut value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    for key in path {
        value = value.get(*key)?;
    }
    Some(value)
}

fn u32_at(value: &Value, path: &[&str]) -> Result<u32, ()> {
    let number = dig(value, path).and_then(Value::as_u64).ok_or(())?;
    u32::try_from(number).map_err(|_| ())
}

fn contribution_count(collection: &Value, repository: Option<&str>) -> Result<u32, ()> {
    if let Some(repository) = repository {
        let mut sum = 0u32;
        for key in CONTRIBUTION_KEYS {
            sum = sum.saturating_add(sum_key(collection, key, Some(repository))?);
        }
        return Ok(sum);
    }
    u32_at(collection, &["contributionCalendar", "totalContributions"])
}

fn review_count(collection: &Value, repository: Option<&str>) -> Result<u32, ()> {
    if let Some(repository) = repository {
        return sum_key(
            collection,
            "pullRequestReviewContributionsByRepository",
            Some(repository),
        );
    }
    u32_at(collection, &["totalPullRequestReviewContributions"])
}

fn sum_key(collection: &Value, key: &str, repository: Option<&str>) -> Result<u32, ()> {
    let items = dig(collection, &[key])
        .and_then(Value::as_array)
        .ok_or(())?;
    let mut sum = 0u32;
    for item in items {
        let Some(name) = dig(item, &["repository", "nameWithOwner"]).and_then(Value::as_str) else {
            continue;
        };
        if !repository_name_ok(name) {
            continue;
        }
        if repository.is_some_and(|repository| !name.eq_ignore_ascii_case(repository)) {
            continue;
        }
        sum = sum.saturating_add(u32_at(item, &["contributions", "totalCount"])?);
    }
    Ok(sum)
}

fn push_names(collection: &Value, names: &mut Vec<String>) -> Result<(), ()> {
    for key in CONTRIBUTION_KEYS {
        let items = dig(collection, &[key])
            .and_then(Value::as_array)
            .ok_or(())?;
        for item in items {
            let Some(name) = dig(item, &["repository", "nameWithOwner"]).and_then(Value::as_str)
            else {
                continue;
            };
            if repository_name_ok(name) {
                names.push(name.to_owned());
            }
        }
    }
    Ok(())
}

fn normalize_names(names: &mut Vec<String>) {
    let mut unique = Vec::new();
    for name in names.iter() {
        if unique
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(name))
        {
            continue;
        }
        unique.push(name.clone());
        if unique.len() == 20 {
            break;
        }
    }
    unique.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    *names = unique;
}

fn parse_weeks(calendar: &Value) -> Result<Vec<GitHubActivityWeek>, ()> {
    let weeks = dig(calendar, &["weeks"])
        .and_then(Value::as_array)
        .ok_or(())?;
    if weeks.len() > 60 {
        return Err(());
    }
    let mut parsed = Vec::with_capacity(weeks.len());
    for week in weeks {
        let days = week
            .get("contributionDays")
            .and_then(Value::as_array)
            .ok_or(())?;
        if days.len() > 7 {
            return Err(());
        }
        let mut items = Vec::with_capacity(days.len());
        for day in days {
            items.push(parse_day(day)?);
        }
        parsed.push(GitHubActivityWeek { days: items });
    }
    Ok(parsed)
}

fn parse_day(value: &Value) -> Result<GitHubActivityDay, ()> {
    let date = value.get("date").and_then(Value::as_str).ok_or(())?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| ())?;
    let weekday = u32_at(value, &["weekday"])?;
    if weekday > 6 {
        return Err(());
    }
    Ok(GitHubActivityDay {
        date: date.to_owned(),
        count: u32_at(value, &["contributionCount"])?,
        level: contribution_level(value)?,
        weekday: weekday as u8,
    })
}

fn contribution_level(value: &Value) -> Result<u8, ()> {
    match value.get("contributionLevel").and_then(Value::as_str) {
        Some("NONE") => Ok(0),
        Some("FIRST_QUARTILE") => Ok(1),
        Some("SECOND_QUARTILE") => Ok(2),
        Some("THIRD_QUARTILE") => Ok(3),
        Some("FOURTH_QUARTILE") => Ok(4),
        _ => Err(()),
    }
}

fn is_weekend(weekday: u8) -> bool {
    weekday == 0 || weekday == 6
}

fn next_date(date: &str) -> Option<String> {
    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let next = parsed + Duration::days(1);
    Some(format!(
        "{:04}-{:02}-{:02}",
        next.year(),
        next.month(),
        next.day()
    ))
}

fn fill_days(days: &[&GitHubActivityDay]) -> Vec<GitHubActivityDay> {
    let mut filled: Vec<GitHubActivityDay> = Vec::with_capacity(days.len());
    for day in days {
        let previous_date = filled.last().map(|item| item.date.clone());
        if let Some(previous_date) = previous_date {
            let parsed = (
                NaiveDate::parse_from_str(&previous_date, "%Y-%m-%d"),
                NaiveDate::parse_from_str(&day.date, "%Y-%m-%d"),
            );
            if let (Ok(mut cursor), Ok(target)) = parsed {
                for _ in 0..400 {
                    let next = cursor + Duration::days(1);
                    if next >= target {
                        break;
                    }
                    filled.push(GitHubActivityDay {
                        date: next.to_string(),
                        count: 0,
                        level: 0,
                        weekday: next.weekday().num_days_from_sunday() as u8,
                    });
                    cursor = next;
                }
            }
        }
        filled.push(GitHubActivityDay::clone(day));
    }
    filled
}

fn current_streak(days: &[&GitHubActivityDay], today: &str) -> u32 {
    let present: Vec<&GitHubActivityDay> = days
        .iter()
        .copied()
        .filter(|day| day.date.as_str() <= today)
        .collect();
    let filled = fill_days(&present);
    let days: Vec<&GitHubActivityDay> = filled.iter().collect();
    let days = days.as_slice();
    let Some(mut index) = days.len().checked_sub(1) else {
        return 0;
    };
    if days[index].date == today && days[index].count == 0 {
        let Some(previous) = index.checked_sub(1) else {
            return 0;
        };
        index = previous;
    }
    let mut streak = 0u32;
    loop {
        let day = days[index];
        if day.count > 0 {
            streak = streak.saturating_add(1);
        } else if !is_weekend(day.weekday) {
            break;
        }
        let Some(previous) = index.checked_sub(1) else {
            break;
        };
        if next_date(&days[previous].date).as_deref() != Some(day.date.as_str()) {
            break;
        }
        index = previous;
    }
    streak
}

fn longest_streak(days: &[&GitHubActivityDay]) -> u32 {
    let filled = fill_days(days);
    let days: Vec<&GitHubActivityDay> = filled.iter().collect();
    let days = days.as_slice();
    let mut best = 0u32;
    let mut current = 0u32;
    let mut previous_date: Option<&str> = None;
    for day in days {
        if previous_date
            .is_some_and(|previous| next_date(previous).as_deref() != Some(day.date.as_str()))
        {
            current = 0;
        }
        if day.count > 0 {
            current = current.saturating_add(1);
            best = best.max(current);
        } else if !is_weekend(day.weekday) {
            current = 0;
        }
        previous_date = Some(day.date.as_str());
    }
    best
}

fn peaks(days: &[&GitHubActivityDay]) -> (Option<GitHubActivityPeak>, Option<GitHubActivityPeak>) {
    let mut months: Vec<(String, u32)> = Vec::new();
    let mut weekdays = [0u32; 7];
    for day in days {
        let Some(key) = day.date.get(0..7) else {
            continue;
        };
        if let Some(entry) = months.iter_mut().find(|entry| entry.0 == key) {
            entry.1 = entry.1.saturating_add(day.count);
        } else {
            months.push((key.to_owned(), day.count));
        }
        if let Some(total) = weekdays.get_mut(usize::from(day.weekday)) {
            *total = total.saturating_add(day.count);
        }
    }
    let month = months
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .max_by_key(|(_, count)| *count)
        .and_then(|(key, count)| {
            month_label(&key).map(|label| GitHubActivityPeak { label, count })
        });
    let mut weekday = None;
    let mut best = 0u32;
    for (index, total) in weekdays.iter().copied().enumerate() {
        if total > best {
            best = total;
            weekday = WEEKDAYS.get(index).map(|label| GitHubActivityPeak {
                label: (*label).to_owned(),
                count: total,
            });
        }
    }
    (month, weekday)
}

fn month_label(year_month: &str) -> Option<String> {
    let year = year_month.get(0..4)?;
    let month: usize = year_month.get(5..7)?.parse().ok()?;
    let name = MONTHS.get(month.checked_sub(1)?)?;
    Some(format!("{name} {year}"))
}

fn attention_url(repository: Option<&str>) -> String {
    match repository {
        None => "https://github.com/pulls/review-requested".to_owned(),
        Some(repository) => {
            let query = format!("is:open is:pr review-requested:@me repo:{repository}");
            format!("https://github.com/pulls?q={}", encode_query(&query))
        }
    }
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len().saturating_mul(3));
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
pub(crate) fn test_activity_payload() -> String {
    let day = |date, count, level, weekday| {
        serde_json::json!({
            "date": date,
            "contributionCount": count,
            "contributionLevel": level,
            "weekday": weekday
        })
    };
    serde_json::json!({
        "data": {
            "viewer": {
                "contributionsCollection": {
                    "contributionCalendar": { "totalContributions": 10 },
                    "totalPullRequestReviewContributions": 0,
                    "commitContributionsByRepository": [{
                        "contributions": { "totalCount": 3 },
                        "repository": { "nameWithOwner": "acme/repo" }
                    }],
                    "issueContributionsByRepository": [],
                    "pullRequestContributionsByRepository": [{
                        "contributions": { "totalCount": 2 },
                        "repository": { "nameWithOwner": "acme/repo" }
                    }],
                    "pullRequestReviewContributionsByRepository": [{
                        "contributions": { "totalCount": 1 },
                        "repository": { "nameWithOwner": "acme/repo" }
                    }]
                },
                "previous": {
                    "contributionCalendar": { "totalContributions": 4 },
                    "totalPullRequestReviewContributions": 0,
                    "commitContributionsByRepository": [],
                    "issueContributionsByRepository": [],
                    "pullRequestContributionsByRepository": [],
                    "pullRequestReviewContributionsByRepository": []
                },
                "year": {
                    "contributionCalendar": {
                        "totalContributions": 42,
                        "weeks": [
                            { "contributionDays": [
                                day("2026-09-01", 1, "FIRST_QUARTILE", 2),
                                day("2026-09-02", 1, "FIRST_QUARTILE", 3),
                                day("2026-09-03", 1, "FIRST_QUARTILE", 4),
                                day("2026-09-04", 0, "NONE", 5)
                            ]},
                            { "contributionDays": [
                                day("2026-10-08", 4, "THIRD_QUARTILE", 4),
                                day("2026-10-09", 5, "FOURTH_QUARTILE", 5),
                                day("2026-10-10", 0, "NONE", 6),
                                day("2026-10-11", 0, "NONE", 0),
                                day("2026-10-12", 0, "NONE", 1)
                            ]}
                        ]
                    },
                    "commitContributionsByRepository": [
                        { "repository": { "nameWithOwner": "acme/repo" } },
                        { "repository": { "nameWithOwner": "acme/other" } },
                        { "repository": { "nameWithOwner": "skip/.." } }
                    ],
                    "issueContributionsByRepository": [],
                    "pullRequestContributionsByRepository": [],
                    "pullRequestReviewContributionsByRepository": []
                }
            },
            "opened": { "issueCount": 5 },
            "openedPrev": { "issueCount": 3 },
            "merged": { "issueCount": 2 },
            "mergedPrev": { "issueCount": 2 },
            "attention": { "issueCount": 1 }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration, Utc};
    use serde_json::Value;
    use zeron_proto::{GitHubActivityDay, GitHubActivityPeriod};

    fn fixed_now() -> DateTime<Utc> {
        "2026-10-12T15:04:05Z".parse().unwrap()
    }

    fn sample_day(date: &str, count: u32, weekday: u8) -> GitHubActivityDay {
        GitHubActivityDay {
            date: date.to_owned(),
            count,
            level: 0,
            weekday,
        }
    }

    #[test]
    fn rolling_windows_are_equal_length() {
        for (period, days) in [
            (GitHubActivityPeriod::Week, 7),
            (GitHubActivityPeriod::Month, 30),
            (GitHubActivityPeriod::Year, 365),
        ] {
            let spec = activity_spec(period, None, fixed_now()).unwrap();
            assert_eq!(spec.to - spec.from, Duration::days(days));
            assert_eq!(spec.from - spec.previous_from, Duration::days(days));
            assert_eq!(spec.previous_to, spec.from);
            assert_eq!(spec.year_to - spec.year_from, Duration::days(365));
            assert_eq!(spec.year_to, spec.to);
        }
        let week = activity_spec(GitHubActivityPeriod::Week, None, fixed_now()).unwrap();
        assert!(week.opened.contains("created:2026-10-06..2026-10-12"));
        assert!(
            week.opened_previous
                .contains("created:2026-09-29..2026-10-05")
        );
        assert!(week.merged.contains("merged:2026-10-06..2026-10-12"));
        assert!(!week.attention.contains("repo:"));
        assert!(ACTIVITY_QUERY.contains("$opened:String!"));
        for (name, _) in week.variables() {
            let marker = String::from("$") + name;
            assert!(ACTIVITY_QUERY.contains(&marker), "{marker}");
        }
        let scoped = activity_spec(
            GitHubActivityPeriod::Week,
            Some("acme/repo".into()),
            fixed_now(),
        )
        .unwrap();
        assert!(scoped.attention.contains("repo:acme/repo"));
        assert!(
            activity_spec(GitHubActivityPeriod::Week, Some("nope".into()), fixed_now()).is_err()
        );
    }

    #[test]
    fn parse_counts_streaks_and_repository_scope() {
        let payload: Value = serde_json::from_str(&test_activity_payload()).unwrap();
        let spec = activity_spec(GitHubActivityPeriod::Week, None, fixed_now()).unwrap();
        let snapshot = parse_activity(&payload, &spec).unwrap();
        assert_eq!(snapshot.contributions.count, 10);
        assert_eq!(snapshot.contributions.delta, 6);
        assert_eq!(snapshot.pull_requests_opened.count, 5);
        assert_eq!(snapshot.pull_requests_opened.delta, 2);
        assert_eq!(snapshot.pull_requests_merged.delta, 0);
        assert_eq!(snapshot.reviews.count, 0);
        assert_eq!(snapshot.reviews.delta, 0);
        assert_eq!(snapshot.year_total, 42);
        assert_eq!(snapshot.current_streak_days, 2);
        assert_eq!(snapshot.longest_streak_days, 3);
        let month = snapshot.most_active_month.as_ref().unwrap();
        assert_eq!(month.label, "Oct 2026");
        assert_eq!(month.count, 9);
        let weekday = snapshot.most_active_weekday.as_ref().unwrap();
        assert_eq!(weekday.label, "Thursdays");
        assert_eq!(weekday.count, 5);
        assert_eq!(
            snapshot.repositories,
            vec!["acme/other".to_owned(), "acme/repo".to_owned()]
        );
        assert_eq!(
            snapshot.attention_url,
            "https://github.com/pulls/review-requested"
        );
        let busy = snapshot
            .weeks
            .iter()
            .flat_map(|week| &week.days)
            .find(|day| day.date == "2026-10-09")
            .unwrap();
        assert_eq!(busy.level, 4);
        assert_eq!(busy.count, 5);
        let scoped = activity_spec(
            GitHubActivityPeriod::Week,
            Some("acme/repo".into()),
            fixed_now(),
        )
        .unwrap();
        let filtered = parse_activity(&payload, &scoped).unwrap();
        assert_eq!(filtered.contributions.count, 6);
        assert_eq!(filtered.contributions.delta, 6);
        assert_eq!(filtered.reviews.count, 1);
        assert_eq!(filtered.pull_requests_opened.count, 5);
        assert!(filtered.attention_url.contains("repo%3Aacme%2Frepo"));
    }

    #[test]
    fn weekends_do_not_break_or_count_and_today_can_be_unfinished() {
        let bridged = vec![
            sample_day("2026-10-09", 1, 5),
            sample_day("2026-10-10", 0, 6),
            sample_day("2026-10-11", 0, 0),
            sample_day("2026-10-12", 1, 1),
        ];
        let refs: Vec<_> = bridged.iter().collect();
        assert_eq!(longest_streak(&refs), 2);
        assert_eq!(current_streak(&refs, "2026-10-12"), 2);
        let unfinished = vec![
            sample_day("2026-10-09", 2, 5),
            sample_day("2026-10-10", 0, 6),
            sample_day("2026-10-11", 0, 0),
            sample_day("2026-10-12", 0, 1),
        ];
        let refs: Vec<_> = unfinished.iter().collect();
        assert_eq!(current_streak(&refs, "2026-10-12"), 1);
        let broken = vec![
            sample_day("2026-10-12", 1, 1),
            sample_day("2026-10-13", 0, 2),
            sample_day("2026-10-14", 4, 3),
        ];
        let refs: Vec<_> = broken.iter().collect();
        assert_eq!(current_streak(&refs, "2026-10-14"), 1);
        assert_eq!(longest_streak(&refs), 1);
        let gapped = vec![
            sample_day("2026-10-01", 5, 4),
            sample_day("2026-10-03", 5, 6),
        ];
        let refs: Vec<_> = gapped.iter().collect();
        assert_eq!(current_streak(&refs, "2026-10-03"), 1);
        assert_eq!(longest_streak(&refs), 1);
        let missing_weekend = vec![
            sample_day("2026-10-09", 1, 5),
            sample_day("2026-10-12", 1, 1),
        ];
        let refs: Vec<_> = missing_weekend.iter().collect();
        assert_eq!(current_streak(&refs, "2026-10-12"), 2);
        assert_eq!(longest_streak(&refs), 2);
    }
    #[test]
    fn future_zero_weekday_does_not_clear_the_current_streak() {
        let days = vec![
            sample_day("2026-10-09", 2, 5),
            sample_day("2026-10-12", 3, 1),
            sample_day("2026-10-13", 0, 2),
        ];
        let refs: Vec<&GitHubActivityDay> = days.iter().collect();
        assert_eq!(current_streak(&refs, "2026-10-12"), 2);
    }

    #[test]
    fn rejects_incomplete_payload() {
        let spec = activity_spec(GitHubActivityPeriod::Week, None, fixed_now()).unwrap();
        assert!(parse_activity(&serde_json::json!({}), &spec).is_err());
    }
}
