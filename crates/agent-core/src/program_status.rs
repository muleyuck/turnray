//! What OSC 7501 (program status) records mean to turnray, for every terminal that reports
//! them: kitty and Rex.

use crate::hook::HookAgent;
use crate::model::Status;

/// The statuses the records map to: `error` shows as Done, so Unknown never appears.
pub const EMITTED: &[Status] = &[Status::Blocked, Status::Done, Status::Idle, Status::Working];

/// An OSC 7501 `state` as one of the app's statuses; `None` for `clear` and anything unknown.
pub fn status(state: &str) -> Option<Status> {
    match state {
        "idle" => Some(Status::Idle),
        "working" => Some(Status::Working),
        "blocked" => Some(Status::Blocked),
        "done" | "error" => Some(Status::Done),
        _ => None,
    }
}

/// The agent a record's `app` names; `None` for any other program.
pub(crate) fn agent(app: &str) -> Option<HookAgent> {
    match app {
        "claude-code" => Some(HookAgent::Claude),
        "codex" => Some(HookAgent::Codex),
        _ => None,
    }
}

/// The workspace a card shows: the last component of the agent's cwd.
pub(crate) fn last_part(path: &str) -> &str {
    path.split('/')
        .rfind(|part| !part.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_map_to_statuses() {
        assert_eq!(status("idle"), Some(Status::Idle));
        assert_eq!(status("working"), Some(Status::Working));
        assert_eq!(status("blocked"), Some(Status::Blocked));
        assert_eq!(status("done"), Some(Status::Done));
        assert_eq!(status("error"), Some(Status::Done));
    }

    #[test]
    fn clear_unknown_and_empty_states_have_no_status() {
        assert_eq!(status("clear"), None);
        assert_eq!(status("sleeping"), None);
        assert_eq!(status(""), None);
    }

    #[test]
    fn only_claude_code_and_codex_are_agents() {
        assert_eq!(agent("claude-code"), Some(HookAgent::Claude));
        assert_eq!(agent("codex"), Some(HookAgent::Codex));
        assert_eq!(agent("cargo"), None);
        assert_eq!(agent(""), None);
    }
}
