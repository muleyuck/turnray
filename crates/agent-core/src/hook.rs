use crate::model::{Agent, Status};
use serde::{Deserialize, Serialize};

/// What Claude Code sends a hook on stdin. Keys not listed here are ignored.
#[derive(Debug, Deserialize)]
pub struct HookEvent {
    pub session_id: String,
    pub cwd: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

impl HookEvent {
    pub fn parse(bytes: &[u8]) -> Result<HookEvent, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// `title: None` keeps the title already on record.
    Write {
        status: Status,
        title: Option<String>,
    },
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAgent {
    Claude,
    Codex,
}

impl HookAgent {
    pub fn parse(s: &str) -> Option<HookAgent> {
        match s {
            "claude" => Some(HookAgent::Claude),
            "codex" => Some(HookAgent::Codex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HookAgent::Claude => "claude",
            HookAgent::Codex => "codex",
        }
    }

    pub fn process_name(self) -> &'static str {
        self.as_str()
    }
}

pub fn decide(agent: HookAgent, event: &HookEvent, current: Option<Status>) -> Option<Action> {
    if event.agent_id.is_some() {
        return None;
    }
    match agent {
        HookAgent::Claude => decide_claude(event, current),
        HookAgent::Codex => decide_codex(event, current),
    }
}

fn write(status: Status) -> Option<Action> {
    Some(Action::Write {
        status,
        title: None,
    })
}

fn decide_claude(event: &HookEvent, current: Option<Status>) -> Option<Action> {
    match event.hook_event_name.as_str() {
        "UserPromptSubmit" => Some(Action::Write {
            status: Status::Working,
            title: event.prompt.as_deref().and_then(title_from_prompt),
        }),
        "PermissionRequest" => write(Status::Blocked),
        "Notification" => match event.notification_type.as_deref() {
            Some("elicitation_dialog" | "agent_needs_input") => write(Status::Blocked),
            Some("idle_prompt") if matches!(current, Some(Status::Working | Status::Blocked)) => {
                write(Status::Idle)
            }
            _ => None,
        },
        "PostToolUse" | "PostToolUseFailure" => write(Status::Working),
        "Stop" => write(Status::Done),
        "SessionStart" => match event.source.as_deref() {
            Some("startup" | "clear" | "resume") => write(Status::Idle),
            _ => None,
        },
        "SessionEnd" => Some(Action::Remove),
        _ => None,
    }
}

fn decide_codex(event: &HookEvent, current: Option<Status>) -> Option<Action> {
    match event.hook_event_name.as_str() {
        "UserPromptSubmit" => Some(Action::Write {
            status: Status::Working,
            title: event.prompt.as_deref().and_then(title_from_prompt),
        }),
        "PermissionRequest" => write(Status::Blocked),
        "PostToolUse" if matches!(current, Some(Status::Working | Status::Blocked)) => {
            write(Status::Working)
        }
        "Stop" => write(Status::Done),
        "Interrupt" => write(Status::Idle),
        "SessionStart" => match event.source.as_deref() {
            Some("startup" | "clear" | "resume") => write(Status::Idle),
            _ => None,
        },
        "SessionEnd" => Some(Action::Remove),
        _ => None,
    }
}

pub fn title_from_prompt(prompt: &str) -> Option<String> {
    prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(80).collect())
}

pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub agent: HookAgent,
    pub status: Status,
    pub cwd: String,
    pub title: Option<String>,
    /// UNIX milliseconds, the start time of the hook that wrote it.
    pub updated_at: u64,
    pub pid: Option<u32>,
    /// `ps -o lstart=` output, trimmed.
    pub pid_start: Option<String>,
}

/// `Status` stays serde-free: the file holds its `as_str` name.
#[derive(Serialize, Deserialize)]
struct RawRecord {
    #[serde(default)]
    agent: Option<String>,
    status: String,
    cwd: String,
    #[serde(default)]
    title: Option<String>,
    updated_at: u64,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    pid_start: Option<String>,
}

impl SessionRecord {
    pub fn to_json(&self) -> String {
        serde_json::to_string(&RawRecord {
            agent: Some(self.agent.as_str().to_string()),
            status: self.status.as_str().to_string(),
            cwd: self.cwd.clone(),
            title: self.title.clone(),
            updated_at: self.updated_at,
            pid: self.pid,
            pid_start: self.pid_start.clone(),
        })
        .expect("a record of strings and numbers always serializes")
    }

    pub fn from_json(text: &str) -> Result<SessionRecord, String> {
        let raw: RawRecord = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let status =
            Status::parse(&raw.status).ok_or_else(|| format!("unknown status {:?}", raw.status))?;
        let agent = match raw.agent.as_deref() {
            None => HookAgent::Claude,
            Some(name) => {
                HookAgent::parse(name).ok_or_else(|| format!("unknown agent {name:?}"))?
            }
        };
        Ok(SessionRecord {
            agent,
            status,
            cwd: raw.cwd,
            title: raw.title,
            updated_at: raw.updated_at,
            pid: raw.pid,
            pid_start: raw.pid_start,
        })
    }

    pub fn to_agent(&self) -> Agent {
        let workspace = self
            .cwd
            .split('/')
            .rfind(|part| !part.is_empty())
            .unwrap_or(&self.cwd);
        Agent {
            status: self.status,
            name: self.agent.as_str().to_string(),
            workspace: workspace.to_string(),
            title: self.title.clone().unwrap_or_default(),
        }
    }
}

/// The statuses the hook source can produce.
pub const EMITTED: &[Status] = &[Status::Blocked, Status::Done, Status::Idle, Status::Working];

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str) -> HookEvent {
        HookEvent {
            session_id: "s1".into(),
            cwd: "/Users/x/proj".into(),
            hook_event_name: name.into(),
            agent_id: None,
            notification_type: None,
            prompt: None,
            source: None,
        }
    }

    fn session_start(source: &str) -> HookEvent {
        HookEvent {
            source: Some(source.into()),
            ..event("SessionStart")
        }
    }

    fn notification(kind: &str) -> HookEvent {
        HookEvent {
            notification_type: Some(kind.into()),
            ..event("Notification")
        }
    }

    fn record(cwd: &str) -> SessionRecord {
        SessionRecord {
            status: Status::Working,
            cwd: cwd.into(),
            title: Some("fix the bug".into()),
            updated_at: 1_700_000_000_123,
            pid: Some(4242),
            pid_start: Some("Sat Oct 31 09:00:00 2026".into()),
            agent: HookAgent::Claude,
        }
    }

    // Built here, not with the production `write`, so a wrong change to that
    // helper cannot change the expectations along with the result.
    fn writes(status: Status) -> Option<Action> {
        Some(Action::Write {
            status,
            title: None,
        })
    }

    fn codex(name: &str, current: Option<Status>) -> Option<Action> {
        decide(HookAgent::Codex, &event(name), current)
    }

    #[test]
    fn a_user_prompt_starts_work_and_takes_the_title_from_the_prompt() {
        let e = HookEvent {
            prompt: Some("fix the bug".into()),
            ..event("UserPromptSubmit")
        };
        assert_eq!(
            decide(HookAgent::Claude, &e, None),
            Some(Action::Write {
                status: Status::Working,
                title: Some("fix the bug".into())
            })
        );
    }

    #[test]
    fn a_permission_request_blocks() {
        assert_eq!(
            decide(HookAgent::Claude, &event("PermissionRequest"), None),
            writes(Status::Blocked)
        );
    }

    #[test]
    fn notifications_that_ask_for_input_block() {
        for kind in ["elicitation_dialog", "agent_needs_input"] {
            assert_eq!(
                decide(HookAgent::Claude, &notification(kind), None),
                writes(Status::Blocked),
                "{kind}"
            );
        }
    }

    #[test]
    fn idle_prompt_idles_only_a_working_or_blocked_session() {
        let idle = notification("idle_prompt");
        assert_eq!(
            decide(HookAgent::Claude, &idle, Some(Status::Working)),
            writes(Status::Idle)
        );
        assert_eq!(
            decide(HookAgent::Claude, &idle, Some(Status::Blocked)),
            writes(Status::Idle)
        );
        assert_eq!(decide(HookAgent::Claude, &idle, Some(Status::Done)), None);
        assert_eq!(decide(HookAgent::Claude, &idle, None), None);
    }

    #[test]
    fn other_notifications_are_ignored() {
        assert_eq!(
            decide(
                HookAgent::Claude,
                &notification("auth_success"),
                Some(Status::Working)
            ),
            None
        );
    }

    #[test]
    fn tool_use_results_mean_working() {
        for name in ["PostToolUse", "PostToolUseFailure"] {
            assert_eq!(
                decide(HookAgent::Claude, &event(name), Some(Status::Blocked)),
                writes(Status::Working),
                "{name}"
            );
        }
    }

    #[test]
    fn stop_means_done() {
        assert_eq!(
            decide(HookAgent::Claude, &event("Stop"), Some(Status::Working)),
            writes(Status::Done)
        );
    }

    #[test]
    fn a_started_cleared_or_resumed_session_is_idle() {
        // A resumed session already has a record; a fresh or cleared one does not.
        for (source, current) in [
            ("startup", None),
            ("clear", None),
            ("resume", Some(Status::Done)),
        ] {
            assert_eq!(
                decide(HookAgent::Claude, &session_start(source), current),
                writes(Status::Idle),
                "{source}"
            );
        }
    }

    #[test]
    fn the_session_start_source_is_read_from_the_payload() {
        let e = HookEvent::parse(
            br#"{"session_id":"s1","cwd":"/x","hook_event_name":"SessionStart","source":"startup","model":"m"}"#,
        )
        .unwrap();
        assert_eq!(e.source.as_deref(), Some("startup"));
    }

    #[test]
    fn a_compaction_does_not_reset_the_status() {
        // Compaction can happen mid-turn; idle would hide a working session.
        assert_eq!(
            decide(
                HookAgent::Claude,
                &session_start("compact"),
                Some(Status::Working)
            ),
            None
        );
    }

    #[test]
    fn session_end_removes_the_record() {
        assert_eq!(
            decide(HookAgent::Claude, &event("SessionEnd"), Some(Status::Done)),
            Some(Action::Remove)
        );
    }

    #[test]
    fn an_unknown_event_is_ignored() {
        assert_eq!(
            decide(
                HookAgent::Claude,
                &event("PreCompact"),
                Some(Status::Working)
            ),
            None
        );
    }

    #[test]
    fn a_subagent_event_is_ignored_whatever_it_is() {
        let e = HookEvent {
            agent_id: Some("a1".into()),
            ..event("Stop")
        };
        assert_eq!(decide(HookAgent::Claude, &e, Some(Status::Working)), None);
    }

    #[test]
    fn the_title_skips_leading_blank_lines_and_trims() {
        assert_eq!(
            title_from_prompt("\n  \n  refactor the parser \nmore detail"),
            Some("refactor the parser".into())
        );
    }

    #[test]
    fn the_title_is_cut_at_80_chars() {
        let at_limit = "a".repeat(80);
        assert_eq!(title_from_prompt(&at_limit), Some(at_limit.clone()));
        assert_eq!(title_from_prompt(&"a".repeat(81)), Some(at_limit));
    }

    #[test]
    fn the_title_is_cut_on_a_char_boundary() {
        assert_eq!(title_from_prompt(&"あ".repeat(81)), Some("あ".repeat(80)));
    }

    #[test]
    fn a_blank_prompt_has_no_title() {
        assert_eq!(title_from_prompt(" \n\t\n"), None);
    }

    #[test]
    fn a_session_id_is_ascii_alphanumerics_and_dashes() {
        assert!(valid_session_id("0b9e1f3a-6c2d-4e8b-9a71-5d3c2f1e0a44"));
        for bad in ["", "../x", "a/b", "a.b"] {
            assert!(!valid_session_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn parse_reads_a_real_user_prompt_submit_payload() {
        let json = br#"{"session_id":"abc-123","transcript_path":"/Users/x/.claude/p/abc.jsonl","cwd":"/Users/x/proj","permission_mode":"default","hook_event_name":"UserPromptSubmit","prompt":"hello"}"#;
        let e = HookEvent::parse(json).unwrap();
        assert_eq!(e.session_id, "abc-123");
        assert_eq!(e.cwd, "/Users/x/proj");
        assert_eq!(e.hook_event_name, "UserPromptSubmit");
        assert_eq!(e.prompt.as_deref(), Some("hello"));
        assert_eq!(e.agent_id, None);
    }

    #[test]
    fn parse_rejects_a_payload_without_an_event_name() {
        assert!(HookEvent::parse(br#"{"session_id":"a","cwd":"/x"}"#).is_err());
    }

    #[test]
    fn parse_rejects_malformed_json() {
        assert!(HookEvent::parse(b"not json").is_err());
    }

    #[test]
    fn a_record_survives_json_round_trip() {
        let r = record("/Users/x/proj");
        assert_eq!(SessionRecord::from_json(&r.to_json()), Ok(r));
    }

    #[test]
    fn a_record_without_optional_fields_round_trips() {
        let r = SessionRecord {
            title: None,
            pid: None,
            pid_start: None,
            ..record("/x")
        };
        assert_eq!(SessionRecord::from_json(&r.to_json()), Ok(r));
    }

    #[test]
    fn a_record_with_an_unknown_status_is_rejected() {
        let json = r#"{"status":"waiting","cwd":"/x","updated_at":1}"#;
        assert!(SessionRecord::from_json(json).is_err());
    }

    #[test]
    fn a_record_missing_a_required_key_is_rejected() {
        assert!(SessionRecord::from_json(r#"{"status":"done","updated_at":1}"#).is_err());
    }

    #[test]
    fn the_agent_is_named_after_the_last_cwd_component() {
        for (cwd, workspace) in [
            ("/Users/x/proj", "proj"),
            ("/Users/x/proj/", "proj"),
            ("/", "/"),
        ] {
            let a = record(cwd).to_agent();
            assert_eq!(a.workspace, workspace, "{cwd}");
            assert_eq!(a.name, "claude");
            assert_eq!(a.status, Status::Working);
            assert_eq!(a.title, "fix the bug");
        }
    }

    #[test]
    fn an_agent_without_a_title_gets_an_empty_one() {
        let r = SessionRecord {
            title: None,
            ..record("/x/y")
        };
        assert_eq!(r.to_agent().title, "");
    }

    #[test]
    fn codex_prompt_starts_work_and_takes_the_title() {
        let e = HookEvent {
            prompt: Some("port the parser".into()),
            ..event("UserPromptSubmit")
        };
        assert_eq!(
            decide(HookAgent::Codex, &e, None),
            Some(Action::Write {
                status: Status::Working,
                title: Some("port the parser".into())
            })
        );
    }

    #[test]
    fn codex_permission_request_blocks() {
        assert_eq!(codex("PermissionRequest", None), writes(Status::Blocked));
    }

    #[test]
    fn codex_post_tool_use_resumes_only_a_working_or_blocked_session() {
        for (current, expected) in [
            (Some(Status::Working), writes(Status::Working)),
            (Some(Status::Blocked), writes(Status::Working)),
            (Some(Status::Done), None),
            (Some(Status::Idle), None),
            (None, None),
        ] {
            assert_eq!(codex("PostToolUse", current), expected, "{current:?}");
        }
    }

    #[test]
    fn codex_stop_means_done() {
        assert_eq!(codex("Stop", Some(Status::Working)), writes(Status::Done));
    }

    #[test]
    fn codex_interrupt_idles_where_claude_ignores_it() {
        assert_eq!(
            codex("Interrupt", Some(Status::Working)),
            writes(Status::Idle)
        );
        assert_eq!(
            decide(
                HookAgent::Claude,
                &event("Interrupt"),
                Some(Status::Working)
            ),
            None
        );
    }

    #[test]
    fn codex_session_end_removes_the_record() {
        assert_eq!(
            codex("SessionEnd", Some(Status::Done)),
            Some(Action::Remove)
        );
    }

    #[test]
    fn codex_session_start_idles_only_for_startup_clear_and_resume() {
        for source in ["startup", "clear", "resume"] {
            assert_eq!(
                decide(HookAgent::Codex, &session_start(source), None),
                writes(Status::Idle),
                "{source}"
            );
        }
        for source in ["compact", "fork"] {
            assert_eq!(
                decide(
                    HookAgent::Codex,
                    &session_start(source),
                    Some(Status::Working)
                ),
                None,
                "{source}"
            );
        }
        assert_eq!(codex("SessionStart", None), None);
    }

    #[test]
    fn codex_ignores_events_only_claude_has() {
        assert_eq!(
            decide(
                HookAgent::Codex,
                &notification("idle_prompt"),
                Some(Status::Working)
            ),
            None
        );
        assert_eq!(codex("PostToolUseFailure", Some(Status::Working)), None);
    }

    #[test]
    fn a_codex_subagent_event_is_ignored() {
        let e = HookEvent {
            agent_id: Some("a1".into()),
            ..event("Stop")
        };
        assert_eq!(decide(HookAgent::Codex, &e, Some(Status::Working)), None);
    }

    #[test]
    fn an_agent_name_parses_back_and_is_case_sensitive() {
        for a in [HookAgent::Claude, HookAgent::Codex] {
            assert_eq!(HookAgent::parse(a.as_str()), Some(a));
            assert_eq!(a.process_name(), a.as_str());
        }
        assert_eq!(HookAgent::parse("cursor"), None);
        assert_eq!(HookAgent::parse("Codex"), None);
    }

    #[test]
    fn a_codex_record_survives_json_round_trip() {
        let r = SessionRecord {
            agent: HookAgent::Codex,
            ..record("/x")
        };
        assert_eq!(SessionRecord::from_json(&r.to_json()), Ok(r));
    }

    #[test]
    fn a_record_without_an_agent_key_is_claude() {
        let json = r#"{"status":"done","cwd":"/x","updated_at":1}"#;
        assert_eq!(
            SessionRecord::from_json(json).unwrap().agent,
            HookAgent::Claude
        );
    }

    #[test]
    fn a_record_with_a_null_agent_is_claude() {
        let json = r#"{"status":"done","cwd":"/x","updated_at":1,"agent":null}"#;
        assert_eq!(
            SessionRecord::from_json(json).unwrap().agent,
            HookAgent::Claude
        );
    }

    #[test]
    fn a_record_with_an_unknown_agent_is_rejected() {
        let json = r#"{"status":"done","cwd":"/x","updated_at":1,"agent":"cursor"}"#;
        assert!(SessionRecord::from_json(json).is_err());
    }

    #[test]
    fn a_codex_record_makes_a_codex_agent() {
        let r = SessionRecord {
            agent: HookAgent::Codex,
            ..record("/x/y")
        };
        assert_eq!(r.to_agent().name, "codex");
    }
}
