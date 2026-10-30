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

pub fn decide(event: &HookEvent, current: Option<Status>) -> Option<Action> {
    if event.agent_id.is_some() {
        return None;
    }
    let write = |status| {
        Some(Action::Write {
            status,
            title: None,
        })
    };
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
        Ok(SessionRecord {
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
            name: "claude".to_string(),
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
        }
    }

    fn notification(kind: &str) -> HookEvent {
        HookEvent {
            notification_type: Some(kind.into()),
            ..event("Notification")
        }
    }

    fn write(status: Status) -> Option<Action> {
        Some(Action::Write {
            status,
            title: None,
        })
    }

    fn record(cwd: &str) -> SessionRecord {
        SessionRecord {
            status: Status::Working,
            cwd: cwd.into(),
            title: Some("fix the bug".into()),
            updated_at: 1_700_000_000_123,
            pid: Some(4242),
            pid_start: Some("Sat Oct 31 09:00:00 2026".into()),
        }
    }

    #[test]
    fn a_user_prompt_starts_work_and_takes_the_title_from_the_prompt() {
        let e = HookEvent {
            prompt: Some("fix the bug".into()),
            ..event("UserPromptSubmit")
        };
        assert_eq!(
            decide(&e, None),
            Some(Action::Write {
                status: Status::Working,
                title: Some("fix the bug".into())
            })
        );
    }

    #[test]
    fn a_permission_request_blocks() {
        assert_eq!(
            decide(&event("PermissionRequest"), None),
            write(Status::Blocked)
        );
    }

    #[test]
    fn notifications_that_ask_for_input_block() {
        for kind in ["elicitation_dialog", "agent_needs_input"] {
            assert_eq!(
                decide(&notification(kind), None),
                write(Status::Blocked),
                "{kind}"
            );
        }
    }

    #[test]
    fn idle_prompt_idles_only_a_working_or_blocked_session() {
        let idle = notification("idle_prompt");
        assert_eq!(decide(&idle, Some(Status::Working)), write(Status::Idle));
        assert_eq!(decide(&idle, Some(Status::Blocked)), write(Status::Idle));
        assert_eq!(decide(&idle, Some(Status::Done)), None);
        assert_eq!(decide(&idle, None), None);
    }

    #[test]
    fn other_notifications_are_ignored() {
        assert_eq!(
            decide(&notification("auth_success"), Some(Status::Working)),
            None
        );
    }

    #[test]
    fn tool_use_results_mean_working() {
        for name in ["PostToolUse", "PostToolUseFailure"] {
            assert_eq!(
                decide(&event(name), Some(Status::Blocked)),
                write(Status::Working),
                "{name}"
            );
        }
    }

    #[test]
    fn stop_means_done() {
        assert_eq!(
            decide(&event("Stop"), Some(Status::Working)),
            write(Status::Done)
        );
    }

    #[test]
    fn session_end_removes_the_record() {
        assert_eq!(
            decide(&event("SessionEnd"), Some(Status::Done)),
            Some(Action::Remove)
        );
    }

    #[test]
    fn an_unknown_event_is_ignored() {
        assert_eq!(decide(&event("PreCompact"), Some(Status::Working)), None);
    }

    #[test]
    fn a_subagent_event_is_ignored_whatever_it_is() {
        let e = HookEvent {
            agent_id: Some("a1".into()),
            ..event("Stop")
        };
        assert_eq!(decide(&e, Some(Status::Working)), None);
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
}
