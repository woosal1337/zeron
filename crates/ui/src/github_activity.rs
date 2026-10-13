use std::time::{Duration, Instant};

use gpui::{Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*, px};
use zeron_proto::{GitHubActivitySnapshot, GitHubActivityStat};
use zeron_rpc::methods;

use crate::state::AppState;
use crate::theme::Theme;

const MIN_WIDTH: f32 = 600.0;
const PANEL_CHROME: f32 = 146.0;
const REFRESH_AFTER: Duration = Duration::from_secs(5 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60);
const IDLE_REFRESH: Duration = Duration::from_secs(30);

fn grouped(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in digits.bytes().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(byte as char);
    }
    out.chars().rev().collect()
}

fn format_delta(delta: i32) -> String {
    let body = grouped(delta.unsigned_abs());
    if delta > 0 {
        format!("+{body}")
    } else if delta < 0 {
        format!("-{body}")
    } else {
        body
    }
}

pub struct GitHubActivity {
    state: Entity<AppState>,
    shown: bool,
    room: f32,
    width: f32,
    snapshot: Option<GitHubActivitySnapshot>,
    fetched_at: Option<Instant>,
    failed_at: Option<Instant>,
    hidden_at: Option<Instant>,
    loading: bool,
}

impl GitHubActivity {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            shown: false,
            room: 0.0,
            width: 0.0,
            snapshot: None,
            fetched_at: None,
            failed_at: None,
            hidden_at: None,
            loading: false,
        }
    }

    pub fn set_frame(&mut self, shown: bool, room: f32, width: f32, cx: &mut Context<Self>) {
        let resumed = shown && !self.shown;
        self.shown = shown;
        self.room = room;
        self.width = width;
        if !shown {
            self.hidden_at.get_or_insert_with(Instant::now);
            return;
        }
        let idle = resumed
            && self
                .hidden_at
                .take()
                .is_some_and(|at| at.elapsed() >= IDLE_REFRESH);
        self.ensure(idle, cx);
    }

    pub fn visible(&self) -> bool {
        self.cell().is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_snapshot(&mut self, snapshot: GitHubActivitySnapshot) {
        self.snapshot = Some(snapshot);
        self.fetched_at = Some(Instant::now());
    }

    fn cell(&self) -> Option<f32> {
        let columns = self.snapshot.as_ref()?.weeks.len();
        self.shown
            .then(|| activity_cell(self.width, columns, self.room))
            .flatten()
    }

    fn ensure(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let fresh = self
            .fetched_at
            .is_some_and(|at| at.elapsed() < REFRESH_AFTER);
        let failed = self.failed_at.is_some_and(|at| at.elapsed() < RETRY_AFTER);
        if !force && (fresh || failed) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let cached = self.snapshot.is_none();
        let refresh = force || !cached;
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::GET_GITHUB_ACTIVITY,
                    serde_json::json!({ "refresh": refresh, "cached": cached }),
                )
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<GitHubActivitySnapshot>(value).ok());
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Some(snapshot) => {
                        this.snapshot = Some(snapshot);
                        this.fetched_at = (!cached).then(Instant::now);
                        this.failed_at = None;
                    }
                    None => this.failed_at = Some(Instant::now()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn delta_color(theme: &Theme, delta: i32) -> gpui::Hsla {
    if delta > 0 {
        theme.success
    } else if delta < 0 {
        theme.danger
    } else {
        theme.text_muted
    }
}

fn stat_column(theme: &Theme, label: &str, stat: GitHubActivityStat) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .whitespace_nowrap()
                .text_size(crate::typography::ui_rems(12.0))
                .line_height(px(16.0))
                .text_color(theme.text_muted)
                .child(SharedString::from(label.to_owned())),
        )
        .child(
            div()
                .whitespace_nowrap()
                .text_size(crate::typography::ui_rems(22.0))
                .line_height(px(26.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(SharedString::from(grouped(stat.count))),
        )
        .child(
            div()
                .h(px(16.0))
                .whitespace_nowrap()
                .text_size(crate::typography::ui_rems(12.0))
                .line_height(px(16.0))
                .text_color(delta_color(theme, stat.delta))
                .when(stat.delta != 0, |row| {
                    row.child(SharedString::from(format_delta(stat.delta)))
                }),
        )
}

fn heat_fill(theme: &Theme, level: u8) -> gpui::Rgba {
    let colors: [u32; 5] = if matches!(theme.appearance, crate::theme::Appearance::Dark) {
        [0x161B22, 0x0E4429, 0x006D32, 0x26A641, 0x39D353]
    } else {
        [0xEBEDF0, 0x9BE9A8, 0x40C463, 0x30A14E, 0x216E39]
    };
    gpui::rgb(colors[usize::from(level.min(4))])
}

fn heat_cell(width: f32, columns: usize) -> f32 {
    let columns = columns.max(1) as f32;
    let gaps = (columns - 1.0).max(0.0) * 2.0;
    ((width - gaps) / columns).max(2.0)
}

fn activity_cell(width: f32, columns: usize, room: f32) -> Option<f32> {
    if width < MIN_WIDTH || columns == 0 {
        return None;
    }
    let cell = heat_cell(width, columns);
    (PANEL_CHROME + 7.0 * cell <= room).then_some(cell)
}

fn month_abbr(date: &str) -> Option<&'static str> {
    let month = date.get(5..7)?.parse::<usize>().ok()?;
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(month.checked_sub(1)?)
    .copied()
}

fn heatmap(snapshot: &GitHubActivitySnapshot, theme: &Theme, cell: f32) -> gpui::Div {
    let month_marks = snapshot.weeks.iter().map(|week| {
        week.days
            .iter()
            .find(|day| day.date.ends_with("-01"))
            .and_then(|day| month_abbr(&day.date))
    });
    div()
        .mt(px(14.0))
        .w_full()
        .child(
            div()
                .flex()
                .gap(px(2.0))
                .mb(px(2.0))
                .children(month_marks.map(|mark| {
                    div()
                        .relative()
                        .w(px(cell))
                        .h(px(14.0))
                        .children(mark.map(|text| {
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .whitespace_nowrap()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(text))
                        }))
                })),
        )
        .child(
            div()
                .flex()
                .gap(px(2.0))
                .children(snapshot.weeks.iter().map(|week| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .children((0..7).map(|row| {
                            let tile = div().w(px(cell)).h(px(cell)).rounded(px(2.0));
                            match week.days.iter().find(|day| day.weekday == row) {
                                Some(day) => tile.bg(heat_fill(theme, day.level)),
                                None => tile,
                            }
                        }))
                })),
        )
}

impl Render for GitHubActivity {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (Some(snapshot), Some(cell)) = (self.snapshot.as_ref(), self.cell()) else {
            return div();
        };
        let theme = Theme::of(cx);
        div()
            .debug_selector(|| "github-activity".into())
            .w_full()
            .child(
                div()
                    .mb(px(4.0))
                    .whitespace_nowrap()
                    .text_size(crate::typography::ui_rems(14.0))
                    .line_height(px(18.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from("Last 30 days")),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_nowrap()
                    .gap(px(16.0))
                    .child(stat_column(theme, "Contributions", snapshot.contributions))
                    .child(stat_column(
                        theme,
                        "PRs opened",
                        snapshot.pull_requests_opened,
                    ))
                    .child(stat_column(
                        theme,
                        "PRs merged",
                        snapshot.pull_requests_merged,
                    ))
                    .child(stat_column(theme, "Reviews", snapshot.reviews)),
            )
            .child(heatmap(snapshot, theme, cell))
    }
}

#[cfg(test)]
mod tests {
    use super::{PANEL_CHROME, activity_cell, format_delta, grouped, heat_cell, month_abbr};

    #[test]
    fn github_activity_groups_counts() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(8368), "8,368");
        assert_eq!(format_delta(0), "0");
        assert_eq!(format_delta(12), "+12");
        assert_eq!(format_delta(-25), "-25");
        assert_eq!(format_delta(1200), "+1,200");
    }

    #[test]
    fn github_activity_shows_only_when_it_fits() {
        let cell = activity_cell(736.0, 53, 400.0).unwrap();
        assert!(PANEL_CHROME + 7.0 * cell <= 400.0);
        assert!(activity_cell(736.0, 53, 200.0).is_none());
        assert!(activity_cell(480.0, 53, 400.0).is_none());
        assert!(activity_cell(736.0, 0, 400.0).is_none());
    }

    #[test]
    fn github_activity_heatmap_uses_the_panel_width() {
        let cell = heat_cell(736.0, 53);
        let used = 53.0 * cell + 52.0 * 2.0;
        assert!((used - 736.0).abs() < 0.5, "{used}");
        assert!(cell > 10.0);
    }

    #[test]
    fn github_activity_month_labels_follow_the_date() {
        assert_eq!(month_abbr("2026-01-02"), Some("Jan"));
        assert_eq!(month_abbr("2026-06-01"), Some("Jun"));
        assert_eq!(month_abbr("2026-12-31"), Some("Dec"));
        assert_eq!(month_abbr("2026-13-01"), None);
        assert_eq!(month_abbr("2026"), None);
    }
}
