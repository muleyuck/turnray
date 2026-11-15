//! Reads the JSON of `kitten @ ls`: the OSC 7501 records of each window become agents.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

use crate::hook::HookAgent;
use crate::model::{Agent, Status};

/// The statuses kitty's records map to: `error` shows as Done, so Unknown never appears.
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

/// A list that is `null` reads as empty, so one odd window cannot fail the whole answer.
fn list<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::deserialize(d)?.unwrap_or_default())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OsWindow {
    #[serde(deserialize_with = "list")]
    tabs: Vec<Tab>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Tab {
    #[serde(deserialize_with = "list")]
    windows: Vec<Window>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Window {
    title: Option<String>,
    cwd: Option<String>,
    #[serde(deserialize_with = "list")]
    foreground_processes: Vec<Process>,
    #[serde(deserialize_with = "list")]
    program_status: Vec<Record>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Process {
    #[serde(deserialize_with = "list")]
    cmdline: Vec<String>,
    cwd: Option<String>,
    pid: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Record {
    id: Option<String>,
    app: Option<String>,
    state: Option<String>,
    title: Option<String>,
}

impl Record {
    fn id(&self) -> &str {
        self.id.as_deref().unwrap_or("")
    }

    fn own_app(&self) -> Option<&str> {
        self.app.as_deref().filter(|a| !a.is_empty())
    }
}

fn hook_agent(app: &str) -> Option<HookAgent> {
    match app {
        "claude-code" => Some(HookAgent::Claude),
        "codex" => Some(HookAgent::Codex),
        _ => None,
    }
}

fn last_part(path: &str) -> &str {
    path.split('/')
        .rfind(|part| !part.is_empty())
        .unwrap_or(path)
}

/// The record's own `app`, else the nearest ancestor's (`a/b/c` -> `a/b`, `a`, the root).
fn effective_app<'a>(record: &'a Record, by_id: &HashMap<&str, &'a Record>) -> Option<&'a str> {
    if let Some(app) = record.own_app() {
        return Some(app);
    }
    let mut id = record.id();
    while !id.is_empty() {
        id = id.rfind('/').map_or("", |i| &id[..i]);
        if let Some(app) = by_id.get(id).and_then(|r| r.own_app()) {
            return Some(app);
        }
    }
    None
}

fn window_agents(window: &Window, out: &mut Vec<Agent>) {
    let by_id: HashMap<&str, &Record> = window.program_status.iter().map(|r| (r.id(), r)).collect();
    for record in &window.program_status {
        let Some(agent) = effective_app(record, &by_id).and_then(hook_agent) else {
            continue;
        };
        let Some(status) = record.state.as_deref().and_then(status) else {
            continue;
        };
        let process = window.foreground_processes.iter().find(|p| {
            p.cmdline
                .first()
                .is_some_and(|c| c.rsplit('/').next() == Some(agent.process_name()))
        });
        let cwd = process
            .and_then(|p| p.cwd.as_deref())
            .or(window.cwd.as_deref())
            .unwrap_or("");
        let title = record
            .title
            .as_deref()
            .filter(|t| !t.is_empty())
            .or(window.title.as_deref())
            .unwrap_or("");
        out.push(Agent {
            status,
            name: agent.as_str().to_string(),
            workspace: last_part(cwd).to_string(),
            title: title.to_string(),
            session_id: None,
            pid: process.and_then(|p| p.pid),
        });
    }
}

/// The agents in one `kitten @ ls` answer, in kitty's order (OS window, tab, window, record).
pub fn parse_ls(json: &str) -> Result<Vec<Agent>, String> {
    let os_windows: Vec<OsWindow> =
        serde_json::from_str(json).map_err(|e| format!("kitten @ ls output: {e}"))?;
    let mut agents = Vec::new();
    for window in os_windows
        .iter()
        .flat_map(|o| &o.tabs)
        .flat_map(|t| &t.windows)
    {
        window_agents(window, &mut agents);
    }
    Ok(agents)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LS: &str = r#"
[
  {
    "id": 1,
    "tabs": [
      {
        "id": 1,
        "title": "emit.sh",
        "windows": [
          {
            "id": 1,
            "title": "emit.sh",
            "cwd": "/Users/x/proj",
            "is_focused": true,
            "foreground_processes": [
              {
                "cmdline": [
                  "/tmp/fixture/claude",
                  "600"
                ],
                "cwd": "/Users/x/proj",
                "pid": 50569
              }
            ],
            "program_status": [
              {
                "app": "claude-code",
                "id": "",
                "kind": null,
                "msg": "Reading files",
                "progress": null,
                "state": "working",
                "title": "Record title ✳"
              },
              {
                "app": "claude-code",
                "id": "sub/a",
                "kind": "permission",
                "msg": "",
                "progress": null,
                "state": "blocked",
                "title": ""
              },
              {
                "app": "cargo",
                "id": "build",
                "kind": null,
                "msg": "",
                "progress": 40,
                "state": "working",
                "title": ""
              }
            ]
          }
        ]
      },
      {
        "id": 2,
        "title": "emit.sh",
        "windows": [
          {
            "id": 2,
            "title": "emit.sh",
            "cwd": "/Users/x/proj",
            "is_focused": true,
            "foreground_processes": [
              {
                "cmdline": [
                  "/tmp/fixture/codex",
                  "600"
                ],
                "cwd": "/Users/x/proj",
                "pid": 50661
              }
            ],
            "program_status": [
              {
                "app": "codex",
                "id": "",
                "kind": null,
                "msg": "Reading files",
                "progress": null,
                "state": "working",
                "title": "Record title ✳"
              },
              {
                "app": "codex",
                "id": "sub/a",
                "kind": "permission",
                "msg": "",
                "progress": null,
                "state": "blocked",
                "title": ""
              },
              {
                "app": "cargo",
                "id": "build",
                "kind": null,
                "msg": "",
                "progress": 40,
                "state": "working",
                "title": ""
              }
            ]
          }
        ]
      },
      {
        "id": 3,
        "title": "sleep",
        "windows": [
          {
            "id": 3,
            "title": "sleep",
            "cwd": "/Users/x/proj",
            "is_focused": true,
            "foreground_processes": [
              {
                "cmdline": [
                  "/bin/sleep",
                  "600"
                ],
                "cwd": "/Users/x/proj",
                "pid": 50679
              }
            ],
            "program_status": []
          }
        ]
      }
    ]
  }
]
"#;

    fn agent(status: Status, name: &str, pid: u32, title: &str) -> Agent {
        Agent {
            status,
            name: name.into(),
            workspace: "proj".into(),
            title: title.into(),
            session_id: None,
            pid: Some(pid),
        }
    }

    /// One window with the given records and a foreground `claude` in `/Users/x/proj`.
    fn ls_with(records: &str) -> String {
        format!(
            r#"[{{"tabs":[{{"windows":[{{"title":"win","cwd":"/Users/x/proj",
            "foreground_processes":[{{"cmdline":["/opt/bin/claude"],"cwd":"/Users/x/proj","pid":77}}],
            "program_status":[{records}]}}]}}]}}]"#
        )
    }

    fn one_window(window_fields: &str) -> String {
        format!(r#"[{{"tabs":[{{"windows":[{{{window_fields}}}]}}]}}]"#)
    }

    #[test]
    fn the_real_capture_gives_four_cards_in_kittys_order() {
        assert_eq!(
            parse_ls(LS).unwrap(),
            vec![
                agent(Status::Working, "claude", 50569, "Record title ✳"),
                agent(Status::Blocked, "claude", 50569, "emit.sh"),
                agent(Status::Working, "codex", 50661, "Record title ✳"),
                agent(Status::Blocked, "codex", 50661, "emit.sh"),
            ]
        );
    }

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
    fn a_child_without_app_takes_its_parents() {
        let json = ls_with(
            r#"{"id":"","app":"codex","state":"idle"},{"id":"a","state":"done"},{"id":"a/b","app":null,"state":"working"}"#,
        );
        let agents = parse_ls(&json).unwrap();
        assert_eq!(
            agents
                .iter()
                .map(|a| (a.name.as_str(), a.status))
                .collect::<Vec<_>>(),
            vec![
                ("codex", Status::Idle),
                ("codex", Status::Done),
                ("codex", Status::Working)
            ]
        );
    }

    #[test]
    fn app_is_found_on_the_nearest_ancestor_that_has_one() {
        let json = ls_with(
            r#"{"state":"idle","app":"claude-code"},{"id":"a","app":"codex","state":"idle"},{"id":"a/b/c","state":"working"}"#,
        );
        let agents = parse_ls(&json).unwrap();
        assert_eq!(agents[2].name, "codex");
    }

    #[test]
    fn records_with_no_app_anywhere_up_the_chain_are_dropped() {
        let json = ls_with(r#"{"id":"","state":"idle"},{"id":"a","state":"working"}"#);
        assert_eq!(parse_ls(&json).unwrap(), vec![]);
    }

    #[test]
    fn a_childs_own_app_wins_over_its_parents() {
        let json = ls_with(
            r#"{"id":"","app":"claude-code","state":"idle"},{"id":"build","app":"cargo","state":"working"}"#,
        );
        let agents = parse_ls(&json).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].status, Status::Idle);
    }

    #[test]
    fn a_root_record_may_omit_id_or_set_it_null() {
        let json = ls_with(
            r#"{"app":"claude-code","state":"idle"},{"id":null,"app":"codex","state":"done"}"#,
        );
        let agents = parse_ls(&json).unwrap();
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn records_with_a_cleared_or_unknown_state_are_dropped() {
        let json = ls_with(
            r#"{"app":"claude-code","state":"clear"},{"id":"a","app":"claude-code","state":"zzz"},{"id":"b","app":"claude-code"}"#,
        );
        assert_eq!(parse_ls(&json).unwrap(), vec![]);
    }

    #[test]
    fn a_record_title_wins_and_an_empty_one_falls_back_to_the_window_title() {
        let json = ls_with(
            r#"{"app":"claude-code","state":"idle","title":"From record"},{"id":"a","app":"claude-code","state":"idle","title":""},{"id":"b","app":"claude-code","state":"idle"}"#,
        );
        let titles: Vec<_> = parse_ls(&json)
            .unwrap()
            .into_iter()
            .map(|a| a.title)
            .collect();
        assert_eq!(titles, vec!["From record", "win", "win"]);
    }

    #[test]
    fn without_a_matching_foreground_process_there_is_no_pid_and_the_window_cwd_names_the_workspace(
    ) {
        let json = one_window(
            r#""title":"t","cwd":"/Users/x/other","foreground_processes":[{"cmdline":["/bin/zsh"],"cwd":"/Users/x/elsewhere","pid":5}],
            "program_status":[{"app":"claude-code","state":"idle"}]"#,
        );
        let agents = parse_ls(&json).unwrap();
        assert_eq!(agents[0].pid, None);
        assert_eq!(agents[0].workspace, "other");
    }

    #[test]
    fn the_matching_process_cwd_names_the_workspace_over_the_window_cwd() {
        let json = one_window(
            r#""cwd":"/work/shell","foreground_processes":[{"cmdline":["/opt/bin/claude"],"cwd":"/work/agent","pid":77}],
            "program_status":[{"app":"claude-code","state":"idle"}]"#,
        );
        assert_eq!(parse_ls(&json).unwrap()[0].workspace, "agent");
    }

    #[test]
    fn a_process_whose_name_only_ends_in_the_agents_is_not_a_match() {
        let json = one_window(
            r#""cwd":"/Users/x/w","foreground_processes":[{"cmdline":["/opt/notclaude"],"cwd":"/Users/x/p","pid":5}],
            "program_status":[{"app":"claude-code","state":"idle"}]"#,
        );
        assert_eq!(parse_ls(&json).unwrap()[0].pid, None);
    }

    #[test]
    fn windows_without_records_give_no_cards() {
        let json = r#"[{"tabs":[{"windows":[{"title":"a"},{"title":"b","program_status":[]}]}]}]"#;
        assert_eq!(parse_ls(json).unwrap(), vec![]);
    }

    #[test]
    fn a_window_with_null_lists_gives_no_cards_and_does_not_hide_the_others() {
        let json = r#"[{"tabs":[{"windows":[
            {"title":"odd","foreground_processes":null,"program_status":null},
            {"title":"ok","cwd":"/Users/x/proj","foreground_processes":[{"cmdline":["claude"],"pid":9}],
             "program_status":[{"app":"claude-code","state":"idle"}]}]}]},{"tabs":null}]"#;
        let agents = parse_ls(json).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].pid, Some(9));
    }

    #[test]
    fn a_trailing_slash_in_cwd_is_ignored_and_root_stays_root() {
        let slash = one_window(
            r#""cwd":"/Users/x/proj/","program_status":[{"app":"codex","state":"idle"}]"#,
        );
        assert_eq!(parse_ls(&slash).unwrap()[0].workspace, "proj");
        let root = one_window(r#""cwd":"/","program_status":[{"app":"codex","state":"idle"}]"#);
        assert_eq!(parse_ls(&root).unwrap()[0].workspace, "/");
    }

    #[test]
    fn output_that_is_not_json_is_an_error() {
        assert!(parse_ls("Error: no remote control").is_err());
    }

    #[test]
    fn a_top_level_object_is_an_error() {
        assert!(parse_ls(r#"{"tabs":[]}"#).is_err());
    }
}
