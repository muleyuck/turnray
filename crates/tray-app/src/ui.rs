//! What to show, worked out from the state alone so it can be tested without a menu bar.

use agent_core::{Agent, Settings, SourceError, Status, Style};

pub const ITEM_MAX_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayState {
    BeforeFirstFetch,
    Ok { agents: Vec<Agent> },
    Error { line: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayImage {
    /// Simple style: the most urgent status
    Status(Status),
    /// Full style: every status that has an agent, with its count drawn in
    Full(Vec<(Status, usize)>),
    /// No agents: something stays on show so the menu can still be opened
    Standby,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub visible: bool,
    pub image: TrayImage,
    pub title: String,
}

impl TrayView {
    fn hidden() -> Self {
        TrayView {
            visible: false,
            image: TrayImage::Full(Vec::new()),
            title: String::new(),
        }
    }
}

pub fn tray_view(state: &DisplayState, settings: &Settings) -> TrayView {
    match state {
        DisplayState::BeforeFirstFetch => TrayView::hidden(),
        // The icon says it failed; a count from before the failure would be a guess.
        DisplayState::Error { .. } => TrayView {
            visible: true,
            image: TrayImage::Error,
            title: String::new(),
        },
        DisplayState::Ok { agents } => {
            let counts = settings.priority.counts(agents);
            let Some(&(top, n)) = counts.first() else {
                return TrayView {
                    visible: true,
                    image: TrayImage::Standby,
                    title: String::new(),
                };
            };
            match settings.style {
                Style::Simple => TrayView {
                    visible: true,
                    image: TrayImage::Status(top),
                    title: n.to_string(),
                },
                Style::Full => TrayView {
                    visible: true,
                    image: TrayImage::Full(counts),
                    title: String::new(),
                },
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorityRow {
    pub status: Status,
    pub label: String,
    pub can_move_up: bool,
    pub can_move_down: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Disabled(String),
    Separator,
    Style(Style),
    Priority(Vec<PriorityRow>),
    Quit,
}

/// The menu line for a failed fetch. A missing herdr says what to do about it.
pub fn error_line(e: &SourceError) -> String {
    match e {
        SourceError::NotFound(msg) => format!("{msg} — install herdr and start it"),
        SourceError::Failed(msg) => msg.clone(),
    }
}

pub fn truncate_label(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Text from herdr is shown as-is, and a terminal title is whatever a program in the pane
/// set. A control character becomes a space so it can't break or blank the line, and a
/// bidi control is dropped so it can't make the line read backwards.
fn displayable(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn agent_label(a: &Agent) -> String {
    let parts: Vec<&str> = [a.name.as_str(), a.workspace.as_str(), a.title.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let label = parts.join(" · ");
    truncate_label(&displayable(&label), ITEM_MAX_CHARS)
}

/// All five, always: a status this source never reports is still listed, marked as such.
pub fn priority_rows(settings: &Settings, emitted: &[Status]) -> Vec<PriorityRow> {
    let order = settings.priority.order();
    order
        .iter()
        .enumerate()
        .map(|(i, &status)| {
            let mut label = format!("{}. {}", i + 1, status.as_str());
            if !emitted.contains(&status) {
                label.push_str(" — not reported by this source");
            }
            PriorityRow {
                status,
                label,
                can_move_up: i > 0,
                can_move_down: i + 1 < order.len(),
            }
        })
        .collect()
}

/// One header per status in priority order, its agents under it.
fn push_agents(entries: &mut Vec<MenuEntry>, agents: &[Agent], settings: &Settings) {
    let mut sorted = agents.to_vec();
    settings.priority.sort(&mut sorted);
    for (status, n) in settings.priority.counts(&sorted) {
        entries.push(MenuEntry::Disabled(format!("{} ({n})", status.as_str())));
        for a in sorted.iter().filter(|a| a.status == status) {
            entries.push(MenuEntry::Disabled(agent_label(a)));
        }
    }
}

pub fn menu_model(state: &DisplayState, settings: &Settings, emitted: &[Status]) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    match state {
        DisplayState::BeforeFirstFetch => {}
        DisplayState::Ok { agents } if agents.is_empty() => {
            entries.push(MenuEntry::Disabled("No agents".into()))
        }
        DisplayState::Ok { agents } => push_agents(&mut entries, agents, settings),
        DisplayState::Error { line } => entries.push(MenuEntry::Disabled(displayable(line))),
    }
    if !entries.is_empty() {
        entries.push(MenuEntry::Separator);
    }
    entries.push(MenuEntry::Style(settings.style));
    entries.push(MenuEntry::Priority(priority_rows(settings, emitted)));
    entries.push(MenuEntry::Separator);
    entries.push(MenuEntry::Quit);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Priority;

    fn agent(status: Status, name: &str, workspace: &str, title: &str) -> Agent {
        Agent {
            status,
            name: name.into(),
            workspace: workspace.into(),
            title: title.into(),
        }
    }

    fn ok(agents: Vec<Agent>) -> DisplayState {
        DisplayState::Ok { agents }
    }

    fn settings(style: Style) -> Settings {
        Settings {
            style,
            priority: Priority::default(),
        }
    }

    fn sample() -> Vec<Agent> {
        vec![
            agent(Status::Working, "claude", "a", ""),
            agent(Status::Idle, "claude", "b", ""),
            agent(Status::Blocked, "codex", "c", ""),
            agent(Status::Idle, "claude", "d", ""),
        ]
    }

    // --- tray_view ---

    #[test]
    fn simple_shows_the_most_urgent_status_and_only_its_count() {
        let v = tray_view(&ok(sample()), &settings(Style::Simple));
        assert!(v.visible);
        assert_eq!(v.image, TrayImage::Status(Status::Blocked));
        assert_eq!(v.title, "1");
    }

    #[test]
    fn simple_falls_to_the_next_status_present() {
        let agents = vec![
            agent(Status::Idle, "a", "", ""),
            agent(Status::Idle, "b", "", ""),
            agent(Status::Working, "c", "", ""),
        ];
        let v = tray_view(&ok(agents), &settings(Style::Simple));
        assert_eq!(v.image, TrayImage::Status(Status::Idle));
        assert_eq!(v.title, "2");
    }

    #[test]
    fn working_alone_is_still_shown() {
        // Nothing on show would read the same as herdr being stopped or broken.
        let agents = vec![
            agent(Status::Working, "a", "", ""),
            agent(Status::Working, "b", "", ""),
        ];
        let v = tray_view(&ok(agents.clone()), &settings(Style::Simple));
        assert_eq!(v.image, TrayImage::Status(Status::Working));
        assert_eq!(v.title, "2");
        let v = tray_view(&ok(agents), &settings(Style::Full));
        assert_eq!(v.image, TrayImage::Full(vec![(Status::Working, 2)]));
    }

    #[test]
    fn full_shows_every_status_present_in_priority_order_with_no_title() {
        let v = tray_view(&ok(sample()), &settings(Style::Full));
        assert!(v.visible);
        assert_eq!(
            v.image,
            TrayImage::Full(vec![
                (Status::Blocked, 1),
                (Status::Idle, 2),
                (Status::Working, 1)
            ])
        );
        assert_eq!(v.title, "");
    }

    #[test]
    fn the_menu_bar_follows_a_changed_priority() {
        let mut s = settings(Style::Simple);
        s.priority = Priority::parse("working,idle,blocked,done,unknown").unwrap();
        assert_eq!(
            tray_view(&ok(sample()), &s).image,
            TrayImage::Status(Status::Working)
        );
    }

    #[test]
    fn no_agents_shows_the_standby_icon_without_a_count() {
        // Hiding the item would also hide the menu, leaving no way to quit.
        for style in [Style::Simple, Style::Full] {
            assert_eq!(
                tray_view(&ok(vec![]), &settings(style)),
                TrayView {
                    visible: true,
                    image: TrayImage::Standby,
                    title: String::new()
                }
            );
        }
    }

    #[test]
    fn nothing_is_shown_before_the_first_fetch() {
        for style in [Style::Simple, Style::Full] {
            assert!(!tray_view(&DisplayState::BeforeFirstFetch, &settings(style)).visible);
        }
    }

    #[test]
    fn an_error_shows_the_error_icon_without_a_count() {
        let v = tray_view(
            &DisplayState::Error { line: "x".into() },
            &settings(Style::Simple),
        );
        assert_eq!(
            v,
            TrayView {
                visible: true,
                image: TrayImage::Error,
                title: String::new()
            }
        );
    }

    // --- menu_model ---

    #[test]
    fn the_list_groups_agents_by_status_in_priority_order() {
        let model = menu_model(&ok(sample()), &settings(Style::Simple), &Status::ALL);
        let lines: Vec<String> = model
            .iter()
            .take_while(|e| **e != MenuEntry::Separator)
            .map(|e| match e {
                MenuEntry::Disabled(s) => s.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            lines,
            [
                "blocked (1)",
                "codex · c",
                "idle (2)",
                "claude · b",
                "claude · d",
                "working (1)",
                "claude · a",
            ]
        );
    }

    #[test]
    fn an_agent_line_names_agent_workspace_and_title() {
        let a = agent(
            Status::Idle,
            "claude",
            "turnray",
            "Herdr agents menubar design",
        );
        let model = menu_model(&ok(vec![a]), &settings(Style::Simple), &Status::ALL);
        assert_eq!(
            model[1],
            MenuEntry::Disabled("claude · turnray · Herdr agents menubar design".to_string())
        );
    }

    #[test]
    fn a_long_agent_line_is_truncated() {
        let a = agent(Status::Idle, "claude", "w", &"x".repeat(100));
        let model = menu_model(&ok(vec![a]), &settings(Style::Simple), &Status::ALL);
        let MenuEntry::Disabled(label) = &model[1] else {
            panic!("expected a line, got {:?}", model[1]);
        };
        assert_eq!(label.chars().count(), ITEM_MAX_CHARS);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn an_agent_line_cannot_be_reshaped_by_its_terminal_title() {
        // Any program in the pane can set the title, so it must not break the line or
        // reverse how the rest of it reads.
        let a = agent(Status::Idle, "claude", "w", "evil\u{202E}txt.exe\nnext");
        let model = menu_model(&ok(vec![a]), &settings(Style::Simple), &Status::ALL);
        assert_eq!(
            model[1],
            MenuEntry::Disabled("claude · w · eviltxt.exe next".to_string())
        );
    }

    #[test]
    fn an_error_line_cannot_carry_control_characters() {
        let model = menu_model(
            &DisplayState::Error {
                line: "herdr not reachable: \u{1b}[31mboom".into(),
            },
            &settings(Style::Simple),
            &Status::ALL,
        );
        assert_eq!(
            model[0],
            MenuEntry::Disabled("herdr not reachable:  [31mboom".into())
        );
    }

    #[test]
    fn a_missing_herdr_says_what_to_do() {
        assert_eq!(
            error_line(&SourceError::NotFound("herdr not found".into())),
            "herdr not found — install herdr and start it"
        );
        assert_eq!(error_line(&SourceError::Failed("boom".into())), "boom");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate_label("あいうえおか", 5), "あいうえ…");
        assert_eq!(truncate_label("あいうえお", 5), "あいうえお");
    }

    #[test]
    fn the_settings_and_quit_are_always_there() {
        for state in [
            DisplayState::BeforeFirstFetch,
            ok(vec![]),
            DisplayState::Error { line: "x".into() },
        ] {
            let model = menu_model(&state, &settings(Style::Full), &Status::ALL);
            let n = model.len();
            assert_eq!(model[n - 4], MenuEntry::Style(Style::Full), "{state:?}");
            assert!(matches!(model[n - 3], MenuEntry::Priority(_)), "{state:?}");
            assert_eq!(model[n - 2], MenuEntry::Separator);
            assert_eq!(model[n - 1], MenuEntry::Quit);
        }
    }

    #[test]
    fn an_error_heads_the_menu() {
        let model = menu_model(
            &DisplayState::Error {
                line: "herdr not found".into(),
            },
            &settings(Style::Simple),
            &Status::ALL,
        );
        assert_eq!(model[0], MenuEntry::Disabled("herdr not found".into()));
        assert_eq!(model[1], MenuEntry::Separator);
    }

    #[test]
    fn no_agents_heads_the_menu_with_a_line_saying_so() {
        let model = menu_model(&ok(vec![]), &settings(Style::Simple), &Status::ALL);
        assert_eq!(model[0], MenuEntry::Disabled("No agents".into()));
        assert_eq!(model[1], MenuEntry::Separator);
    }

    #[test]
    fn priority_rows_list_all_five_and_disable_moves_past_the_ends() {
        let rows = priority_rows(&settings(Style::Simple), &Status::ALL);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "1. blocked",
                "2. done",
                "3. idle",
                "4. working",
                "5. unknown"
            ]
        );
        assert!(!rows[0].can_move_up && rows[0].can_move_down);
        assert!(rows[2].can_move_up && rows[2].can_move_down);
        assert!(rows[4].can_move_up && !rows[4].can_move_down);
    }

    #[test]
    fn a_status_the_source_never_reports_is_marked_not_dropped() {
        let rows = priority_rows(
            &settings(Style::Simple),
            &[Status::Blocked, Status::Done, Status::Working],
        );
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[2].label, "3. idle — not reported by this source");
        assert_eq!(rows[3].label, "4. working");
    }
}
