//! Reads the JSON of `herdr agent list` and `herdr workspace list`.

use std::collections::HashMap;

use serde::Deserialize;

use crate::model::{Agent, Status};

/// herdr's `AgentStatus` is exactly the app's five.
pub const EMITTED: &[Status] = &Status::ALL;

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct AgentList {
    #[serde(default)]
    agents: Vec<RawAgent>,
}

#[derive(Deserialize)]
struct RawAgent {
    #[serde(default)]
    agent_status: String,
    #[serde(default)]
    pane_id: String,
    #[serde(default)]
    display_agent: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    terminal_title_stripped: Option<String>,
    #[serde(default)]
    terminal_title: Option<String>,
}

#[derive(Deserialize)]
struct WorkspaceList {
    #[serde(default)]
    workspaces: Vec<RawWorkspace>,
}

#[derive(Deserialize)]
struct RawWorkspace {
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    label: Option<String>,
}

/// The value is the current state itself, so one we don't know is honestly "unknown".
pub fn status(raw: &str) -> Status {
    Status::parse(raw).unwrap_or(Status::Unknown)
}

fn first_non_empty(candidates: [&Option<String>; 2]) -> String {
    candidates
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_default()
}

/// Workspace labels keyed by id, from `herdr workspace list`.
pub fn parse_workspaces(json: &str) -> Result<HashMap<String, String>, String> {
    let list: Envelope<WorkspaceList> =
        serde_json::from_str(json).map_err(|e| format!("unreadable workspace list: {e}"))?;
    Ok(list
        .result
        .workspaces
        .into_iter()
        .filter_map(|w| match w.label {
            Some(label) if !label.is_empty() => Some((w.workspace_id, label)),
            _ => None,
        })
        .collect())
}

/// Agents from `herdr agent list`, in herdr's order. A workspace without a label shows
/// its id, so the line still says where the agent is.
pub fn parse_agents(json: &str, labels: &HashMap<String, String>) -> Result<Vec<Agent>, String> {
    let list: Envelope<AgentList> =
        serde_json::from_str(json).map_err(|e| format!("unreadable agent list: {e}"))?;
    Ok(list
        .result
        .agents
        .into_iter()
        .filter(|a| !a.pane_id.is_empty())
        .map(|a| Agent {
            status: status(&a.agent_status),
            name: first_non_empty([&a.display_agent, &a.agent]),
            workspace: labels
                .get(&a.workspace_id)
                .cloned()
                .unwrap_or_else(|| a.workspace_id.clone()),
            title: first_non_empty([&a.terminal_title_stripped, &a.terminal_title]),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from real `herdr agent list` / `herdr workspace list` output.
    const AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[
        {"agent":"claude","agent_status":"idle","focused":false,"pane_id":"w19:p1",
         "terminal_title":"✳ Vue学習ロードマップ作成","terminal_title_stripped":"Vue学習ロードマップ作成","workspace_id":"w19"},
        {"agent":"claude","agent_status":"working","focused":true,"pane_id":"w1J:p4",
         "terminal_title":"◐ Herdr agents menubar design","terminal_title_stripped":"Herdr agents menubar design","workspace_id":"w1J"}
        ],"type":"agent_list"}}"#;
    const WORKSPACES: &str = r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[
        {"label":"react-vue-comparison","workspace_id":"w19"},
        {"label":"turnray","workspace_id":"w1J"}]}}"#;

    fn labels() -> HashMap<String, String> {
        parse_workspaces(WORKSPACES).unwrap()
    }

    #[test]
    fn real_output_parses_into_agents_with_workspace_labels() {
        assert_eq!(
            parse_agents(AGENTS, &labels()).unwrap(),
            vec![
                Agent {
                    status: Status::Idle,
                    name: "claude".into(),
                    workspace: "react-vue-comparison".into(),
                    title: "Vue学習ロードマップ作成".into(),
                },
                Agent {
                    status: Status::Working,
                    name: "claude".into(),
                    workspace: "turnray".into(),
                    title: "Herdr agents menubar design".into(),
                },
            ]
        );
    }

    #[test]
    fn known_statuses_pass_through_and_anything_else_is_unknown() {
        for st in Status::ALL {
            assert_eq!(status(st.as_str()), st);
        }
        assert_eq!(status("sleeping"), Status::Unknown);
        assert_eq!(status(""), Status::Unknown);
    }

    #[test]
    fn agents_without_a_pane_are_dropped() {
        let json = r#"{"result":{"agents":[
            {"agent":"claude","agent_status":"idle","pane_id":"","workspace_id":"w1"},
            {"agent":"codex","agent_status":"done","pane_id":"w1:p2","workspace_id":"w1"}]}}"#;
        let agents = parse_agents(json, &HashMap::new()).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "codex");
    }

    #[test]
    fn display_name_and_stripped_title_win_when_present() {
        let json = r#"{"result":{"agents":[
            {"agent":"claude","display_agent":"Claude Code","agent_status":"idle","pane_id":"p",
             "terminal_title":"✳ raw","terminal_title_stripped":"clean","workspace_id":"w"}]}}"#;
        let a = &parse_agents(json, &HashMap::new()).unwrap()[0];
        assert_eq!(a.name, "Claude Code");
        assert_eq!(a.title, "clean");
    }

    #[test]
    fn empty_or_missing_preferred_fields_fall_back() {
        let json = r#"{"result":{"agents":[
            {"agent":"claude","display_agent":"","agent_status":"idle","pane_id":"p",
             "terminal_title":"raw","terminal_title_stripped":"","workspace_id":"w9"},
            {"agent":"codex","agent_status":"idle","pane_id":"q","workspace_id":"w9"}]}}"#;
        let agents = parse_agents(json, &HashMap::new()).unwrap();
        assert_eq!(agents[0].name, "claude");
        assert_eq!(agents[0].title, "raw");
        // No label known: the id still says where it is.
        assert_eq!(agents[0].workspace, "w9");
        assert_eq!(agents[1].title, "");
    }

    #[test]
    fn an_empty_label_is_not_used() {
        let json = r#"{"result":{"workspaces":[{"label":"","workspace_id":"w1"}]}}"#;
        assert!(parse_workspaces(json).unwrap().is_empty());
    }

    #[test]
    fn output_that_is_not_the_expected_json_is_an_error() {
        assert!(parse_agents("Error: no server", &HashMap::new()).is_err());
        assert!(parse_workspaces("{}").is_err());
    }
}
