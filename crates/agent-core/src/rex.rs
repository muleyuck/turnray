//! Reads what turnray's Lua script returns through `rex do`: the OSC 7501 records of
//! each Rex terminal block become agents.

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::hook::HookAgent;
use crate::kitty::{hook_agent, last_part};
use crate::model::{Agent, Identity, Status};

/// Rex reads the same records as kitty, so it maps them to the same statuses.
pub const EMITTED: &[Status] = crate::kitty::EMITTED;

/// A list that is `null`, missing or `{}` reads as empty: Rex's Lua writes an empty table
/// as `{}`.
fn list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) if map.is_empty() => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value).map_err(D::Error::custom),
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
struct Blocks(#[serde(deserialize_with = "list")] Vec<Block>);

#[derive(Deserialize, Default)]
#[serde(default)]
struct Block {
    process: Processes,
    program_status: ProgramStatus,
    title: Title,
}

/// The block method `process`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Processes {
    foreground: Option<Process>,
    child: Option<Process>,
    #[serde(deserialize_with = "list")]
    ancestors: Vec<Process>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Process {
    pid: Option<u32>,
    cwd: Option<String>,
}

/// The block method `program_status`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ProgramStatus {
    #[serde(deserialize_with = "list")]
    records: Vec<Record>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Record {
    /// Empty or missing on the root record; a child's names it.
    id: Option<String>,
    /// The record's `app`, or the one it inherits from its parent record.
    effective_app: Option<String>,
    state: Option<String>,
    title: Option<String>,
    owner: Option<Owner>,
}

/// The process that sent the record.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Owner {
    pid: Option<u32>,
}

/// The block method `title`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Title {
    title: Option<String>,
}

/// One card per agent in the block. Rex adds a child record for each command the agent
/// runs, so the records are one agent: the card is the root record's (no `id`), which
/// holds the agent's own state, else the first record's.
fn block_agents(block: &Block, out: &mut Vec<Agent>) {
    let processes = &block.process;
    let mut groups: Vec<(HookAgent, Vec<(&Record, Status)>)> = Vec::new();
    for record in &block.program_status.records {
        let Some(agent) = record.effective_app.as_deref().and_then(hook_agent) else {
            continue;
        };
        let Some(status) = record.state.as_deref().and_then(crate::kitty::status) else {
            continue;
        };
        match groups.iter_mut().find(|(a, _)| *a == agent) {
            Some((_, records)) => records.push((record, status)),
            None => groups.push((agent, vec![(record, status)])),
        }
    }
    for (agent, records) in groups {
        let (root, status) = *records
            .iter()
            .find(|(r, _)| r.id.as_deref().unwrap_or("").is_empty())
            .unwrap_or(&records[0]);
        let pid = std::iter::once(root)
            .chain(records.iter().map(|(r, _)| *r))
            .find_map(|r| r.owner.as_ref().and_then(|o| o.pid));
        let owner = processes
            .foreground
            .iter()
            .chain(&processes.child)
            .chain(&processes.ancestors)
            .find(|p| pid.is_some() && p.pid == pid);
        let cwd = owner
            .or(processes.foreground.as_ref())
            .and_then(|p| p.cwd.as_deref())
            .unwrap_or("");
        let title = root
            .title
            .as_deref()
            .filter(|t| !t.is_empty())
            .or(block.title.title.as_deref())
            .unwrap_or("");
        out.push(Agent {
            status,
            name: agent.as_str().to_string(),
            workspace: last_part(cwd).to_string(),
            title: title.to_string(),
            identities: pid.map(Identity::Pid).into_iter().collect(),
        });
    }
}

/// The agents in the script's answer, in Rex's order (session, window, layer, block).
pub fn parse(json: &str) -> Result<Vec<Agent>, String> {
    let Blocks(blocks) = serde_json::from_str(json).map_err(|e| format!("rex do output: {e}"))?;
    let mut agents = Vec::new();
    for block in &blocks {
        block_agents(block, &mut agents);
    }
    Ok(agents)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real answer of the script, with paths and titles replaced.
    const ANSWER: &str = r#"
[
  {
    "process": {
      "ancestors": [
        {
          "argv0": "-zsh",
          "cwd": "/Users/x/work",
          "invoked_path": "/bin/zsh",
          "name": "zsh",
          "path": "/bin/zsh",
          "pid": 82334,
          "user": "x",
          "user_id": 501
        },
        {
          "name": "login",
          "path": "/usr/bin/login",
          "pid": 82331,
          "user": "root",
          "user_id": 0
        }
      ],
      "child": {
        "name": "login",
        "path": "/usr/bin/login",
        "pid": 82331,
        "user": "root",
        "user_id": 0
      },
      "foreground": {
        "argv0": "claude",
        "cwd": "/Users/x/work",
        "invoked_path": "/opt/homebrew/bin/claude",
        "name": "claude",
        "path": "/opt/homebrew/Caskroom/claude-code@latest/2.1.330/claude",
        "pid": 82741,
        "user": "x",
        "user_id": 501
      },
      "home": "/Users/x"
    },
    "program_status": {
      "records": [
        {
          "app": "claude-code",
          "effective_app": "claude-code",
          "owner": {
            "name": "claude",
            "pid": 82741
          },
          "state": "done",
          "updated_at": "2026-11-21T22:05:15.144535+09:00"
        }
      ]
    },
    "title": {
      "owner": {
        "name": "claude",
        "pid": 82741
      },
      "title": "Session title ✳"
    }
  },
  {
    "process": {
      "child": {
        "argv0": "claude",
        "cwd": "/Users/x/proj",
        "invoked_path": "/opt/homebrew/bin/claude",
        "name": "claude",
        "path": "/opt/homebrew/Caskroom/claude-code@latest/2.1.330/claude",
        "pid": 89517,
        "user": "x",
        "user_id": 501
      },
      "foreground": {
        "argv0": "claude",
        "cwd": "/Users/x/proj",
        "invoked_path": "/opt/homebrew/bin/claude",
        "name": "claude",
        "path": "/opt/homebrew/Caskroom/claude-code@latest/2.1.330/claude",
        "pid": 89517,
        "user": "x",
        "user_id": 501
      },
      "home": "/Users/x"
    },
    "program_status": {
      "records": [
        {
          "app": "claude-code",
          "effective_app": "claude-code",
          "owner": {
            "name": "claude",
            "pid": 89517
          },
          "state": "working",
          "updated_at": "2026-11-21T22:05:11.113352+09:00"
        },
        {
          "app": "claude-code",
          "effective_app": "claude-code",
          "id": "x11bwrjcb",
          "owner": {
            "name": "claude",
            "pid": 89517
          },
          "state": "working",
          "title": "started by Claude",
          "updated_at": "2026-11-21T22:07:44.813603+09:00"
        }
      ]
    },
    "title": {
      "owner": {
        "name": "claude",
        "pid": 89517
      },
      "title": "Block title"
    }
  }
]
"#;

    fn agent(status: Status, workspace: &str, pid: u32, title: &str) -> Agent {
        Agent {
            status,
            name: "claude".into(),
            workspace: workspace.into(),
            title: title.into(),
            identities: vec![Identity::Pid(pid)],
        }
    }

    /// One block with the given records and a foreground `claude` (pid 77) in `/Users/x/proj`.
    fn block_with(records: &str) -> String {
        format!(
            r#"[{{"process":{{"foreground":{{"name":"claude","cwd":"/Users/x/proj","pid":77}}}},
            "program_status":{{"records":[{records}]}},"title":{{"title":"blk"}}}}]"#
        )
    }

    fn statuses(json: &str) -> Vec<Status> {
        parse(json).unwrap().into_iter().map(|a| a.status).collect()
    }

    #[test]
    fn the_real_capture_gives_a_card_per_block_in_rexs_order() {
        assert_eq!(
            parse(ANSWER).unwrap(),
            vec![
                agent(Status::Done, "work", 82741, "Session title ✳"),
                agent(Status::Working, "proj", 89517, "Block title"),
            ]
        );
    }

    #[test]
    fn an_empty_answer_is_written_as_an_object_and_gives_no_cards() {
        assert_eq!(parse("{}\n").unwrap(), vec![]);
        assert_eq!(parse("[]").unwrap(), vec![]);
    }

    #[test]
    fn empty_records_written_as_an_object_give_no_cards() {
        let json = r#"[{"program_status":{"records":{}}},{"program_status":{"records":null}},{}]"#;
        assert_eq!(parse(json).unwrap(), vec![]);
    }

    #[test]
    fn codex_records_become_codex_cards_and_other_apps_are_dropped() {
        let json = block_with(
            r#"{"effective_app":"codex","state":"idle","owner":{"pid":77}},{"id":"b","effective_app":"cargo","state":"working"},{"id":"c","app":"codex","state":"blocked"}"#,
        );
        let agents = parse(&json).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(
            (agents[0].name.as_str(), agents[0].status),
            ("codex", Status::Idle)
        );
    }

    #[test]
    fn states_map_as_kittys_and_cleared_or_unknown_ones_are_dropped() {
        let one = |state: &str| {
            statuses(&block_with(&format!(
                r#"{{"effective_app":"claude-code","state":"{state}"}}"#
            )))
        };
        assert_eq!(one("error"), [Status::Done]);
        assert_eq!(one("blocked"), [Status::Blocked]);
        assert_eq!(one("clear"), []);
        assert_eq!(one("zzz"), []);
        assert_eq!(
            statuses(&block_with(r#"{"effective_app":"claude-code"}"#)),
            []
        );
    }

    #[test]
    fn a_blocks_records_are_one_card_with_the_root_records_state_and_title() {
        let json = block_with(
            r#"{"effective_app":"claude-code","state":"done","title":"Root"},{"id":"a","effective_app":"claude-code","state":"working","title":"started by Claude"},{"id":"b","effective_app":"claude-code","state":"blocked"}"#,
        );
        let agents = parse(&json).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].status, Status::Done);
        assert_eq!(agents[0].title, "Root");
    }

    #[test]
    fn the_root_record_need_not_come_first() {
        let json = block_with(
            r#"{"id":"a","effective_app":"claude-code","state":"working"},{"effective_app":"claude-code","state":"idle"}"#,
        );
        assert_eq!(statuses(&json), [Status::Idle]);
    }

    #[test]
    fn a_cleared_root_leaves_the_first_childs_state() {
        let json = block_with(
            r#"{"effective_app":"claude-code","state":"clear"},{"id":"a","effective_app":"claude-code","state":"working"},{"id":"b","effective_app":"claude-code","state":"blocked"}"#,
        );
        assert_eq!(statuses(&json), [Status::Working]);
    }

    #[test]
    fn a_cleared_child_does_not_hide_the_root() {
        let json = block_with(
            r#"{"effective_app":"claude-code","state":"done"},{"id":"a","effective_app":"claude-code","state":"clear"}"#,
        );
        assert_eq!(statuses(&json), [Status::Done]);
    }

    #[test]
    fn without_a_root_record_the_first_child_gives_state_title_and_pid() {
        let json = block_with(
            r#"{"id":"a","effective_app":"claude-code","state":"idle","title":"First","owner":{"pid":77}},{"id":"b","effective_app":"claude-code","state":"working","title":"Second","owner":{"pid":78}}"#,
        );
        let agents = parse(&json).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].status, Status::Idle);
        assert_eq!(agents[0].title, "First");
        assert_eq!(agents[0].identities, [Identity::Pid(77)]);
    }

    #[test]
    fn claude_and_codex_in_one_block_are_two_cards() {
        let json = block_with(
            r#"{"effective_app":"claude-code","state":"idle"},{"id":"a","effective_app":"codex","state":"working"}"#,
        );
        let names: Vec<_> = parse(&json).unwrap().into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["claude", "codex"]);
    }

    #[test]
    fn an_empty_root_title_falls_back_to_the_block_title() {
        for root in [
            r#"{"effective_app":"claude-code","state":"idle","title":""}"#,
            r#"{"effective_app":"claude-code","state":"idle"}"#,
        ] {
            assert_eq!(parse(&block_with(root)).unwrap()[0].title, "blk");
        }
        let titled =
            block_with(r#"{"effective_app":"claude-code","state":"idle","title":"From record"}"#);
        assert_eq!(parse(&titled).unwrap()[0].title, "From record");
    }

    #[test]
    fn the_pid_is_the_records_owner_and_its_cwd_names_the_workspace() {
        let json = r#"[{"process":{"foreground":{"name":"rg","cwd":"/work/shell","pid":5},
            "ancestors":[{"name":"claude","cwd":"/work/agent","pid":77}]},
            "program_status":{"records":[{"effective_app":"claude-code","state":"working","owner":{"pid":77}}]}}]"#;
        let agents = parse(json).unwrap();
        assert_eq!(agents[0].identities, [Identity::Pid(77)]);
        assert_eq!(agents[0].workspace, "agent");
    }

    #[test]
    fn without_an_owner_there_is_no_pid_and_the_foreground_cwd_names_the_workspace() {
        let json = r#"[{"process":{"foreground":{"name":"claude","cwd":"/work/fg","pid":5}},
            "program_status":{"records":[{"effective_app":"claude-code","state":"idle"}]}}]"#;
        let agents = parse(json).unwrap();
        assert_eq!(agents[0].identities, []);
        assert_eq!(agents[0].workspace, "fg");
    }

    #[test]
    fn output_that_is_not_json_is_an_error() {
        assert!(parse("Error: no Rex server is running").is_err());
    }

    #[test]
    fn a_non_empty_top_level_object_is_an_error() {
        assert!(parse(r#"{"blocks":[]}"#).is_err());
    }
}
