//! What to show, worked out from the state alone so it can be tested without a menu bar
//! or a panel.

use agent_core::{Agent, Settings, SourceError, Status, Style};

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
    /// No agents: something stays on show so the panel can still be opened
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
pub enum PanelContent {
    /// Before the first fetch. The tray is hidden then, so the panel can't be opened
    Empty,
    /// One group per status that has an agent, in priority order
    Groups(Vec<PanelGroup>),
    /// No agents, or a failed fetch: one line in place of the list
    Message { text: String, warning: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelGroup {
    pub status: Status,
    pub count: usize,
    pub agents: Vec<AgentCard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCard {
    /// Agent and workspace, the empty ones left out
    pub heading: String,
    /// The terminal title; `None` when it is empty, so the card has no second line
    pub title: Option<String>,
}

/// What the ⚙ button's menu offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsMenu {
    pub style: Style,
    pub priority: Vec<PriorityRow>,
}

/// A rectangle in screen points, with AppKit's bottom-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Between the menu bar and the panel's top.
pub const PANEL_GAP: f64 = 4.0;
/// Kept between the panel and the screen's sides.
pub const PANEL_MARGIN: f64 = 8.0;

/// Where the panel goes: its top left just under the icon's left edge, as a menu opens,
/// moved only as far as it takes to stay on `visible` (the screen less the menu bar).
pub fn panel_frame(icon: Rect, width: f64, height: f64, visible: Rect) -> Rect {
    let rightmost = visible.x + visible.width - PANEL_MARGIN - width;
    let x = icon.x.min(rightmost).max(visible.x + PANEL_MARGIN);
    Rect {
        x,
        y: icon.y - PANEL_GAP - height,
        width,
        height,
    }
}

/// The panel line for a failed fetch. A missing herdr says what to do about it.
pub fn error_line(e: &SourceError) -> String {
    match e {
        SourceError::NotFound(msg) => format!("{msg} — install herdr and start it"),
        SourceError::Failed(msg) => msg.clone(),
    }
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

fn agent_card(a: &Agent) -> AgentCard {
    let parts: Vec<&str> = [a.name.as_str(), a.workspace.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let heading = displayable(&parts.join(" · "));
    let title = displayable(&a.title);
    AgentCard {
        // herdr can leave both out, and a blank bold line reads as a drawing fault.
        heading: if heading.trim().is_empty() {
            "unknown agent".into()
        } else {
            heading
        },
        title: (!title.trim().is_empty()).then_some(title),
    }
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

pub fn panel_model(state: &DisplayState, settings: &Settings) -> PanelContent {
    match state {
        DisplayState::BeforeFirstFetch => PanelContent::Empty,
        DisplayState::Ok { agents } if agents.is_empty() => PanelContent::Message {
            text: "No agents".into(),
            warning: false,
        },
        DisplayState::Ok { agents } => {
            let mut sorted = agents.to_vec();
            settings.priority.sort(&mut sorted);
            let groups = settings
                .priority
                .counts(&sorted)
                .into_iter()
                .map(|(status, count)| PanelGroup {
                    status,
                    count,
                    agents: sorted
                        .iter()
                        .filter(|a| a.status == status)
                        .map(agent_card)
                        .collect(),
                })
                .collect();
            PanelContent::Groups(groups)
        }
        DisplayState::Error { line } => PanelContent::Message {
            text: displayable(line),
            warning: true,
        },
    }
}

pub fn settings_model(settings: &Settings, emitted: &[Status]) -> SettingsMenu {
    SettingsMenu {
        style: settings.style,
        priority: priority_rows(settings, emitted),
    }
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
        // Hiding the item would also hide the panel, leaving no way to quit.
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

    // --- panel_model ---

    fn groups(content: PanelContent) -> Vec<PanelGroup> {
        match content {
            PanelContent::Groups(groups) => groups,
            other => panic!("expected groups, got {other:?}"),
        }
    }

    fn card(heading: &str, title: Option<&str>) -> AgentCard {
        AgentCard {
            heading: heading.into(),
            title: title.map(Into::into),
        }
    }

    #[test]
    fn the_panel_groups_agents_by_status_in_priority_order() {
        let groups = groups(panel_model(&ok(sample()), &settings(Style::Simple)));
        assert_eq!(
            groups,
            vec![
                PanelGroup {
                    status: Status::Blocked,
                    count: 1,
                    agents: vec![card("codex · c", None)],
                },
                PanelGroup {
                    status: Status::Idle,
                    count: 2,
                    agents: vec![card("claude · b", None), card("claude · d", None)],
                },
                PanelGroup {
                    status: Status::Working,
                    count: 1,
                    agents: vec![card("claude · a", None)],
                },
            ]
        );
    }

    #[test]
    fn the_panel_follows_a_changed_priority() {
        let mut s = settings(Style::Simple);
        s.priority = Priority::parse("working,idle,blocked,done,unknown").unwrap();
        let order: Vec<Status> = groups(panel_model(&ok(sample()), &s))
            .iter()
            .map(|g| g.status)
            .collect();
        assert_eq!(order, [Status::Working, Status::Idle, Status::Blocked]);
    }

    #[test]
    fn a_card_puts_agent_and_workspace_first_and_the_title_second() {
        let a = agent(
            Status::Idle,
            "claude",
            "turnray",
            "Herdr agents menubar design",
        );
        let groups = groups(panel_model(&ok(vec![a]), &settings(Style::Simple)));
        assert_eq!(
            groups[0].agents,
            [card(
                "claude · turnray",
                Some("Herdr agents menubar design")
            )]
        );
    }

    #[test]
    fn an_empty_workspace_is_left_out_of_the_heading() {
        let a = agent(Status::Idle, "claude", "", "t");
        let groups = groups(panel_model(&ok(vec![a]), &settings(Style::Simple)));
        assert_eq!(groups[0].agents, [card("claude", Some("t"))]);
    }

    #[test]
    fn a_long_title_is_kept_whole_for_the_panel_to_truncate() {
        let title = "x".repeat(200);
        let a = agent(Status::Idle, "claude", "w", &title);
        let groups = groups(panel_model(&ok(vec![a]), &settings(Style::Simple)));
        assert_eq!(groups[0].agents[0].title.as_deref(), Some(title.as_str()));
    }

    #[test]
    fn a_card_cannot_be_reshaped_by_its_terminal_title() {
        // Any program in the pane can set the title, so it must not break the line or
        // reverse how the rest of it reads.
        let a = agent(
            Status::Idle,
            "claude",
            "w\u{202E}x",
            "evil\u{202E}txt.exe\nnext",
        );
        let groups = groups(panel_model(&ok(vec![a]), &settings(Style::Simple)));
        assert_eq!(
            groups[0].agents,
            [card("claude · wx", Some("eviltxt.exe next"))]
        );
    }

    #[test]
    fn no_agents_is_a_message_without_a_warning() {
        assert_eq!(
            panel_model(&ok(vec![]), &settings(Style::Simple)),
            PanelContent::Message {
                text: "No agents".into(),
                warning: false
            }
        );
    }

    #[test]
    fn an_error_is_a_warning_message_without_control_characters() {
        assert_eq!(
            panel_model(
                &DisplayState::Error {
                    line: "herdr not reachable: \u{1b}[31mboom".into(),
                },
                &settings(Style::Simple),
            ),
            PanelContent::Message {
                text: "herdr not reachable:  [31mboom".into(),
                warning: true
            }
        );
    }

    #[test]
    fn the_panel_is_empty_before_the_first_fetch() {
        assert_eq!(
            panel_model(&DisplayState::BeforeFirstFetch, &settings(Style::Simple)),
            PanelContent::Empty
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
    fn a_card_with_neither_agent_nor_workspace_says_so() {
        let a = agent(Status::Idle, "", "", "t");
        let groups = groups(panel_model(&ok(vec![a]), &settings(Style::Simple)));
        assert_eq!(groups[0].agents, [card("unknown agent", Some("t"))]);
    }

    #[test]
    fn a_title_of_only_control_characters_leaves_no_second_line() {
        let a = agent(Status::Idle, "claude", "w", "\u{202E}\n");
        let groups = groups(panel_model(&ok(vec![a]), &settings(Style::Simple)));
        assert_eq!(groups[0].agents, [card("claude · w", None)]);
    }

    // --- panel_frame ---

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1728.0,
        height: 1080.0,
    };

    fn icon_at(x: f64) -> Rect {
        Rect {
            x,
            y: 1080.0,
            width: 30.0,
            height: 24.0,
        }
    }

    #[test]
    fn the_panel_hangs_from_the_icons_left_edge() {
        assert_eq!(
            panel_frame(icon_at(1000.0), 320.0, 300.0, SCREEN),
            Rect {
                x: 1000.0,
                y: 1080.0 - PANEL_GAP - 300.0,
                width: 320.0,
                height: 300.0
            }
        );
    }

    #[test]
    fn near_the_right_edge_the_panel_moves_left_to_stay_on_screen() {
        let f = panel_frame(icon_at(1700.0), 320.0, 300.0, SCREEN);
        assert_eq!(f.x, 1728.0 - PANEL_MARGIN - 320.0);
    }

    #[test]
    fn on_a_screen_left_of_the_main_one_the_panel_stays_on_it() {
        let screen = Rect {
            x: -1440.0,
            ..SCREEN
        };
        let f = panel_frame(icon_at(-1440.0), 320.0, 300.0, screen);
        assert_eq!(f.x, -1440.0 + PANEL_MARGIN);
    }

    // --- settings_model ---

    #[test]
    fn the_settings_menu_carries_the_style_and_all_five_priorities() {
        let m = settings_model(&settings(Style::Full), &Status::ALL);
        assert_eq!(m.style, Style::Full);
        assert_eq!(
            m.priority,
            priority_rows(&settings(Style::Full), &Status::ALL)
        );
        assert_eq!(m.priority.len(), 5);
    }

    #[test]
    fn the_settings_menu_marks_what_the_source_never_reports() {
        let m = settings_model(
            &settings(Style::Simple),
            &[Status::Blocked, Status::Done, Status::Working],
        );
        assert_eq!(m.priority.len(), 5);
        assert_eq!(m.priority[2].label, "3. idle — not reported by this source");
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
