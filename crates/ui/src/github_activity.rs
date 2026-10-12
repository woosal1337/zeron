//! GitHub activity under the empty new-chat composer.
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, IntoElement, Render, ScrollHandle, SharedString, Window, div, prelude::*, px,
};
use zeron_proto::{
    GitHubActivityPeak, GitHubActivityPeriod, GitHubActivitySnapshot, GitHubActivityStat,
};
use zeron_rpc::methods;

use crate::popover;
use crate::state::AppState;
use crate::theme::Theme;

pub(crate) const ACTIVITY_MIN_ROOM: f32 = 150.0;
const REFRESH_AFTER: Duration = Duration::from_secs(5 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60);
const IDLE_REFRESH: Duration = Duration::from_secs(30);

pub(crate) fn activity_mounts(anchor: f32, room: f32, has_snapshot: bool) -> bool {
    anchor > 40.0 && room >= ACTIVITY_MIN_ROOM && has_snapshot
}

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

fn day_count(days: u32) -> String {
    if days == 1 {
        "1 day".to_owned()
    } else {
        format!("{days} days")
    }
}

fn period_label(period: GitHubActivityPeriod) -> &'static str {
    match period {
        GitHubActivityPeriod::Week => "This week",
        GitHubActivityPeriod::Month => "This month",
        GitHubActivityPeriod::Year => "This year",
    }
}

fn delta_caption(period: GitHubActivityPeriod) -> &'static str {
    match period {
        GitHubActivityPeriod::Week => "vs last week",
        GitHubActivityPeriod::Month => "vs last month",
        GitHubActivityPeriod::Year => "vs last year",
    }
}

fn repo_label(repository: &Option<String>) -> String {
    match repository {
        Some(repository) => repository.clone(),
        None => "All repositories".to_owned(),
    }
}

pub struct GitHubActivity {
    state: Entity<AppState>,
    anchor_height: Rc<Cell<f32>>,
    panel_width: Rc<Cell<f32>>,
    shown: bool,
    room: f32,
    period: GitHubActivityPeriod,
    repository: Option<String>,
    snapshot: Option<GitHubActivitySnapshot>,
    fetched_key: Option<String>,
    fetched_at: Option<Instant>,
    failed_at: Option<Instant>,
    hidden_at: Option<Instant>,
    loading: bool,
    loading_key: Option<String>,
    generation: u64,
    period_open: bool,
    repo_open: bool,
    scroll: ScrollHandle,
    repo_scroll: ScrollHandle,
}

impl GitHubActivity {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            anchor_height: Rc::new(Cell::new(0.0)),
            panel_width: Rc::new(Cell::new(680.0)),
            shown: false,
            room: 0.0,
            period: GitHubActivityPeriod::Week,
            repository: None,
            snapshot: None,
            fetched_key: None,
            fetched_at: None,
            failed_at: None,
            hidden_at: None,
            loading: false,
            loading_key: None,
            generation: 0,
            period_open: false,
            repo_open: false,
            scroll: ScrollHandle::new(),
            repo_scroll: ScrollHandle::new(),
        }
    }

    pub fn anchor_height(&self) -> Rc<Cell<f32>> {
        self.anchor_height.clone()
    }

    pub fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    pub fn set_frame(&mut self, shown: bool, room: f32, cx: &mut Context<Self>) {
        let resumed = shown && !self.shown;
        self.shown = shown;
        self.room = room;
        if !shown || room < ACTIVITY_MIN_ROOM {
            self.period_open = false;
            self.repo_open = false;
        }
        if !shown {
            if self.hidden_at.is_none() {
                self.hidden_at = Some(Instant::now());
            }
            return;
        }
        let idle = resumed
            && self
                .hidden_at
                .take()
                .is_some_and(|at| at.elapsed() >= IDLE_REFRESH);
        self.hidden_at = None;
        self.ensure(idle, cx);
    }

    fn request_key(&self) -> String {
        format!(
            "{:?}:{}",
            self.period,
            self.repository.as_deref().unwrap_or("")
        )
    }

    fn ensure(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let key = self.request_key();
        if self.loading && self.loading_key.as_deref() == Some(key.as_str()) && !force {
            return;
        }
        let same = self.fetched_key.as_deref() == Some(key.as_str());
        let fresh = self
            .fetched_at
            .is_some_and(|at| at.elapsed() < REFRESH_AFTER);
        if !force && same && self.snapshot.is_some() && fresh {
            return;
        }
        if !force && self.failed_at.is_some_and(|at| at.elapsed() < RETRY_AFTER) {
            return;
        }
        self.fetch(force || (same && self.snapshot.is_some()), key, cx);
    }

    fn fetch(&mut self, refresh: bool, key: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let period = self.period;
        let repository = self.repository.clone();
        self.loading = true;
        self.loading_key = Some(key.clone());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let mut params = serde_json::json!({
                "period": period,
                "refresh": refresh,
            });
            if let Some(repository) = repository {
                params["repository"] = serde_json::Value::String(repository);
            }
            let result = engine
                .client()
                .call(methods::GET_GITHUB_ACTIVITY, params)
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(value) => match serde_json::from_value::<GitHubActivitySnapshot>(value) {
                        Ok(snapshot) => {
                            this.snapshot = Some(snapshot);
                            this.fetched_key = Some(key);
                            this.fetched_at = Some(Instant::now());
                            this.failed_at = None;
                        }
                        Err(_) => this.failed_at = Some(Instant::now()),
                    },
                    Err(_) => this.failed_at = Some(Instant::now()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_period(&mut self, cx: &mut Context<Self>) {
        self.period_open = !self.period_open;
        if self.period_open {
            self.repo_open = false;
        }
        cx.notify();
    }

    fn toggle_repo(&mut self, cx: &mut Context<Self>) {
        self.repo_open = !self.repo_open;
        if self.repo_open {
            self.period_open = false;
        }
        cx.notify();
    }

    fn choose_period(&mut self, period: GitHubActivityPeriod, cx: &mut Context<Self>) {
        self.period_open = false;
        self.repo_open = false;
        if self.period != period {
            self.period = period;
            self.ensure(true, cx);
        }
        cx.notify();
    }

    fn choose_repository(&mut self, repository: Option<String>, cx: &mut Context<Self>) {
        self.period_open = false;
        self.repo_open = false;
        if self.repository != repository {
            self.repository = repository;
            self.ensure(true, cx);
        }
        cx.notify();
    }

    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let period = self.period;
        let repository = self.repository.clone();
        let names = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.repositories.clone())
            .unwrap_or_default();
        div()
            .flex()
            .items_center()
            .justify_between()
            .mb(px(8.0))
            .child(
                div()
                    .text_size(crate::typography::ui_rems(14.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from("Overview")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(self.period_menu(theme, period, cx))
                    .child(self.repo_menu(theme, &repository, &names, cx)),
            )
    }

    fn period_menu(
        &self,
        theme: &Theme,
        period: GitHubActivityPeriod,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let open = self.period_open;
        div()
            .relative()
            .child(self.trigger(
                theme,
                period_label(period),
                "github-activity-period",
                cx,
                true,
            ))
            .when(open, |menu| {
                menu.child(
                    div().absolute().top(px(26.0)).right_0().occlude().child(
                        popover::popover_card(theme).min_w(px(148.0)).children(
                            [
                                GitHubActivityPeriod::Week,
                                GitHubActivityPeriod::Month,
                                GitHubActivityPeriod::Year,
                            ]
                            .map(|item| {
                                let selected = item == period;
                                div()
                                    .px(px(8.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .cursor_pointer()
                                    .bg(if selected {
                                        theme.surface_raised
                                    } else {
                                        gpui::transparent_black()
                                    })
                                    .child(SharedString::from(period_label(item)))
                                    .id(match item {
                                        GitHubActivityPeriod::Week => "github-activity-week",
                                        GitHubActivityPeriod::Month => "github-activity-month",
                                        GitHubActivityPeriod::Year => "github-activity-year",
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_period(item, cx);
                                    }))
                            }),
                        ),
                    ),
                )
            })
    }

    fn repo_menu(
        &self,
        theme: &Theme,
        repository: &Option<String>,
        names: &[String],
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let open = self.repo_open;
        let label = repo_label(repository);
        let selected_name = repository.clone();
        div()
            .relative()
            .child(self.trigger(theme, &label, "github-activity-repo", cx, false))
            .when(open, |menu| {
                let rows = std::iter::once(None::<String>)
                    .chain(names.iter().cloned().map(Some))
                    .map(|name| {
                        let selected = name == selected_name;
                        let shown = repo_label(&name);
                        div()
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .bg(if selected {
                                theme.surface_raised
                            } else {
                                gpui::transparent_black()
                            })
                            .child(SharedString::from(shown))
                            .id(SharedString::from(format!(
                                "github-activity-repo-{}",
                                name.as_deref().unwrap_or("all")
                            )))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.choose_repository(name.clone(), cx);
                            }))
                            .into_any_element()
                    });
                menu.child(
                    div().absolute().top(px(26.0)).right_0().occlude().child(
                        popover::popover_card(theme).min_w(px(220.0)).child(
                            popover::menu_scroll_list("github-activity-repos", &self.repo_scroll)
                                .max_h(px(240.0))
                                .flex()
                                .flex_col()
                                .children(rows),
                        ),
                    ),
                )
            })
    }

    fn trigger(
        &self,
        theme: &Theme,
        label: &str,
        id: &'static str,
        cx: &mut Context<Self>,
        period: bool,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(4.0))
            .max_w(px(200.0))
            .cursor_pointer()
            .text_size(crate::typography::ui_rems(12.0))
            .text_color(theme.text_muted)
            .child(SharedString::from(format!("{label} ▾")))
            .on_click(cx.listener(move |this, _, _, cx| {
                if period {
                    this.toggle_period(cx);
                } else {
                    this.toggle_repo(cx);
                }
            }))
    }
}

fn delta_color(theme: &Theme, delta: i32) -> gpui::Hsla {
    if delta > 0 {
        theme.success
    } else {
        theme.text_muted
    }
}

fn stat_row(theme: &Theme, label: &str, stat: GitHubActivityStat, caption: &str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(22.0))
        .child(
            div()
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(theme.text_muted)
                .child(SharedString::from(label.to_owned())),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(grouped(stat.count))),
                )
                .when(stat.delta != 0, |row| {
                    row.child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(if stat.delta > 0 {
                                theme.success
                            } else {
                                theme.danger
                            }),
                    )
                })
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(delta_color(theme, stat.delta))
                        .child(SharedString::from(format!(
                            "{} {caption}",
                            format_delta(stat.delta)
                        ))),
                ),
        )
}

fn metric(theme: &Theme, label: &str, value: String, detail: Option<String>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child(SharedString::from(label.to_owned())),
        )
        .child(
            div()
                .text_size(crate::typography::ui_rems(14.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(SharedString::from(value)),
        )
        .children(detail.map(|detail| {
            div()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(SharedString::from(detail))
        }))
}

fn peak_parts(peak: &Option<GitHubActivityPeak>) -> (String, Option<String>) {
    match peak {
        Some(peak) => (
            peak.label.clone(),
            Some(format!("{} contributions", grouped(peak.count))),
        ),
        None => ("—".to_owned(), None),
    }
}

fn theme_dark(theme: &Theme) -> bool {
    matches!(theme.appearance, crate::theme::Appearance::Dark)
}

fn heat_fill(dark: bool, level: u8) -> gpui::Rgba {
    let colors: [u32; 5] = if dark {
        [0x161B22, 0x0E4429, 0x006D32, 0x26A641, 0x39D353]
    } else {
        [0xEBEDF0, 0x9BE9A8, 0x40C463, 0x30A14E, 0x216E39]
    };
    gpui::rgb(colors[usize::from(level.min(4))])
}

fn heat_cell(width: f32, columns: usize) -> f32 {
    let columns = columns.max(1) as f32;
    ((width - 44.0) / columns - 2.0).clamp(3.0, 10.0)
}

fn heatmap(snapshot: &GitHubActivitySnapshot, theme: &Theme, width: f32) -> gpui::Div {
    let columns = snapshot.weeks.len();
    if columns == 0 {
        return div();
    }
    let cell = heat_cell(width, columns);
    let dark = theme_dark(theme);
    let label = |text: &str| {
        div()
            .h(px(cell))
            .flex()
            .items_center()
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text_muted)
            .child(SharedString::from(text.to_owned()))
    };
    div()
        .mt(px(8.0))
        .child(
            div()
                .flex()
                .gap(px(4.0))
                .child(
                    div()
                        .w(px(28.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(label(""))
                        .child(label("Mon"))
                        .child(label(""))
                        .child(label("Wed"))
                        .child(label(""))
                        .child(label("Fri"))
                        .child(label("")),
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
                                    let day = week.days.iter().find(|day| day.weekday == row);
                                    let mut tile = div().w(px(cell)).h(px(cell)).rounded(px(2.0));
                                    if let Some(day) = day {
                                        tile = tile.bg(heat_fill(dark, day.level));
                                    }
                                    tile
                                }))
                        })),
                ),
        )
        .child(
            div()
                .mt(px(6.0))
                .flex()
                .justify_end()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from("Less")),
                )
                .children((0..5).map(|level| {
                    div()
                        .w(px(10.0))
                        .h(px(10.0))
                        .rounded(px(2.0))
                        .bg(heat_fill(dark, level))
                }))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from("More")),
                ),
        )
}

impl Render for GitHubActivity {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = if self.shown && self.room >= ACTIVITY_MIN_ROOM {
            self.snapshot.clone()
        } else {
            None
        };
        let Some(snapshot) = snapshot else {
            return div().id("github-activity");
        };
        let theme = Theme::of(cx).clone();
        let key = self.request_key();
        let stale = self.loading && self.fetched_key.as_deref() != Some(key.as_str());
        let width = self.panel_width.clone();
        let caption = delta_caption(snapshot.period);
        let (month_value, month_detail) = peak_parts(&snapshot.most_active_month);
        let (day_value, day_detail) = peak_parts(&snapshot.most_active_weekday);
        let attention_url = snapshot.attention_url.clone();
        let attention_count = grouped(snapshot.attention_count);
        div()
            .id("github-activity")
            .relative()
            .w_full()
            .opacity(if stale { 0.55 } else { 1.0 })
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.period_open || this.repo_open {
                    this.period_open = false;
                    this.repo_open = false;
                    cx.notify();
                }
            }))
            .child(
                gpui::canvas(
                    move |bounds, window, _cx| {
                        let next = f32::from(bounds.size.width);
                        if next > 1.0 && (width.get() - next).abs() > 1.0 {
                            width.set(next);
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(self.header(&theme, cx))
            .child(
                div()
                    .id("github-activity-scroll")
                    .max_h(px((self.room - 48.0).max(80.0)))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(stat_row(
                        &theme,
                        "Contributions",
                        snapshot.contributions,
                        caption,
                    ))
                    .child(stat_row(
                        &theme,
                        "Pull requests opened",
                        snapshot.pull_requests_opened,
                        caption,
                    ))
                    .child(stat_row(
                        &theme,
                        "Pull requests merged",
                        snapshot.pull_requests_merged,
                        caption,
                    ))
                    .child(stat_row(&theme, "Reviews", snapshot.reviews, caption))
                    .child(
                        div()
                            .mt(px(12.0))
                            .text_size(crate::typography::ui_rems(13.0))
                            .text_color(theme.text)
                            .child(SharedString::from(format!(
                                "{} contributions in the last year",
                                grouped(snapshot.year_total)
                            ))),
                    )
                    .child(heatmap(&snapshot, &theme, self.panel_width.get()))
                    .child(
                        div()
                            .mt(px(14.0))
                            .flex()
                            .gap(px(16.0))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.0))
                                    .child(metric(
                                        &theme,
                                        "Current streak",
                                        day_count(snapshot.current_streak_days),
                                        Some("Weekends don't break it".to_owned()),
                                    ))
                                    .child(metric(
                                        &theme,
                                        "Longest streak",
                                        day_count(snapshot.longest_streak_days),
                                        None,
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.0))
                                    .child(metric(
                                        &theme,
                                        "Most active month",
                                        month_value,
                                        month_detail,
                                    ))
                                    .child(metric(
                                        &theme,
                                        "Most active day",
                                        day_value,
                                        day_detail,
                                    )),
                            ),
                    )
                    .when(snapshot.attention_count > 0, |body| {
                        let url = attention_url.clone();
                        body.child(div().mt(px(12.0)).h(px(1.0)).bg(theme.border))
                            .child(
                                div()
                                    .mt(px(8.0))
                                    .mb(px(4.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .text_size(crate::typography::ui_rems(13.0))
                                            .text_color(theme.text)
                                            .child(SharedString::from("Needs your attention")),
                                    )
                                    .child(
                                        div()
                                            .text_size(crate::typography::ui_rems(13.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text)
                                            .child(SharedString::from(attention_count)),
                                    )
                                    .id("github-activity-attention")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.open_url(&url);
                                    })),
                            )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{activity_mounts, day_count, format_delta, grouped};

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
        assert_eq!(day_count(1), "1 day");
        assert_eq!(day_count(52), "52 days");
    }

    #[test]
    fn github_activity_mounts_with_room_and_snapshot() {
        assert!(!activity_mounts(41.0, 150.0, false));
        assert!(!activity_mounts(40.0, 400.0, true));
        assert!(!activity_mounts(80.0, 149.0, true));
        assert!(activity_mounts(80.0, 150.0, true));
    }
}
