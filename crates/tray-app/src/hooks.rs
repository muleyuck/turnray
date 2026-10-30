//! The hook side of the Claude Code source: `turnray hook claude` turns one hook event
//! from stdin into a session file under `sessions_dir()`.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_core::hook::{decide, valid_session_id, Action, HookEvent, SessionRecord, EMITTED};
use agent_core::{Agent, DataSource, SourceError, Status};

use crate::process::output_with_timeout;

/// Stdin kept by the hook. A `PostToolUse` carries the whole `tool_response`, which can be
/// large, but anything past this is not a hook event worth parsing.
const MAX_INPUT: usize = 64 * 1024 * 1024;

/// How many processes up from the hook to look for `claude`.
const ANCESTOR_DEPTH: usize = 3;

const PS_TIMEOUT: Duration = Duration::from_secs(2);

pub fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/turnray/sessions"))
}

/// `write_event_with` for a caller that already has the owner.
#[cfg(test)]
pub fn write_event(
    dir: &Path,
    input: &[u8],
    now_ms: u64,
    owner: Option<(u32, String)>,
) -> Result<(), String> {
    write_event_with(dir, input, now_ms, || owner)
}

fn write_event_with(
    dir: &Path,
    input: &[u8],
    now_ms: u64,
    owner: impl FnOnce() -> Option<(u32, String)>,
) -> Result<(), String> {
    let event = HookEvent::parse(input)?;
    // The id becomes a file name, so it is checked before anything touches the disk.
    if !valid_session_id(&event.session_id) {
        return Err(format!("invalid session_id {:?}", event.session_id));
    }
    let path = dir.join(format!("{}.json", event.session_id));
    let read = || {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| SessionRecord::from_json(&text).ok())
    };
    let existing = read();
    match decide(&event, existing.as_ref().map(|r| r.status)) {
        None => Ok(()),
        Some(Action::Remove) => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
        Some(Action::Write { status, title }) => {
            // Async hooks can overtake each other; the newer record stays.
            if existing.as_ref().is_some_and(|r| r.updated_at > now_ms) {
                return Ok(());
            }
            let (pid, pid_start) = match owner() {
                Some((pid, start)) => (Some(pid), Some(start)),
                None => (None, None),
            };
            // The owner lookup runs `ps`, long enough for a newer hook to land meanwhile.
            // Checking again narrows that window; there is no lock.
            let existing = read();
            if existing.as_ref().is_some_and(|r| r.updated_at > now_ms) {
                return Ok(());
            }
            let record = SessionRecord {
                status,
                cwd: event.cwd,
                title: title.or_else(|| existing.and_then(|r| r.title)),
                updated_at: now_ms,
                pid,
                pid_start,
            };
            write_atomic(dir, &path, &event.session_id, &record.to_json())
        }
    }
}

/// Writes through a temp file in the same directory, so a reader never sees half a record.
fn write_atomic(dir: &Path, path: &Path, id: &str, text: &str) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{id}.{}.tmp", std::process::id()));
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}

pub fn run_hook() {
    let herdr_env = std::env::var("HERDR_ENV").is_ok_and(|v| v == "1");
    let result = match sessions_dir() {
        Some(dir) => handle(
            &dir,
            std::io::stdin().lock(),
            herdr_env,
            now_ms(),
            find_claude,
            MAX_INPUT,
        ),
        None => {
            // Still drain stdin, as handle would, so Claude is not left writing to a dead pipe.
            let _ = read_capped(std::io::stdin().lock(), MAX_INPUT);
            Err("HOME is not set".to_string())
        }
    };
    if let Err(e) = result {
        eprintln!("turnray hook: {e}");
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn handle(
    dir: &Path,
    reader: impl Read,
    herdr_env: bool,
    now_ms: u64,
    owner: impl FnOnce() -> Option<(u32, String)>,
    cap: usize,
) -> Result<(), String> {
    // Read to the end first: a Claude writing to a pipe nobody reads blocks or gets EPIPE.
    let input = read_capped(reader, cap).ok_or("input too large")?;
    if herdr_env {
        return Ok(());
    }
    write_event_with(dir, &input, now_ms, owner)
}

fn read_capped(mut reader: impl Read, cap: usize) -> Option<Vec<u8>> {
    let mut kept = Vec::new();
    let mut over = false;
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let room = cap - kept.len();
                kept.extend_from_slice(&buf[..n.min(room)]);
                over |= n > room;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    (!over).then_some(kept)
}

/// The nearest `claude` among the hook's ancestors, as (pid, lstart).
fn find_claude() -> Option<(u32, String)> {
    find_claude_from(std::os::unix::process::parent_id(), ps_ancestor)
}

/// (ppid, comm, lstart) of `pid` from `ps`.
fn ps_ancestor(pid: u32) -> Option<(u32, String, String)> {
    let out = output_with_timeout(
        Command::new("ps").env("LC_ALL", "C").args([
            "-o",
            "ppid=,comm=,lstart=",
            "-p",
            &pid.to_string(),
        ]),
        PS_TIMEOUT,
    )
    .ok()?;
    parse_ps_ancestor(&String::from_utf8_lossy(&out.stdout))
}

fn find_claude_from(
    start: u32,
    lookup: impl Fn(u32) -> Option<(u32, String, String)>,
) -> Option<(u32, String)> {
    let mut pid = start;
    for _ in 0..ANCESTOR_DEPTH {
        let (ppid, comm, lstart) = lookup(pid)?;
        if Path::new(&comm).file_name().is_some_and(|n| n == "claude") {
            return Some((pid, lstart));
        }
        pid = ppid;
    }
    None
}

/// `comm` may contain spaces, but `lstart` is always the last five fields.
fn parse_ps_ancestor(line: &str) -> Option<(u32, String, String)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 7 {
        return None;
    }
    let ppid = fields[0].parse().ok()?;
    let split = fields.len() - 5;
    Some((ppid, fields[1..split].join(" "), fields[split..].join(" ")))
}

/// A record with no owner (`claude` was not found among the hook's ancestors) can only
/// age out.
const NO_OWNER_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// The app side of the Claude Code source: reads the session files the hook wrote and
/// removes the ones whose `claude` is gone.
// wired into main in the next task
#[allow(dead_code)]
pub struct HookSource {
    dir: Option<PathBuf>,
    /// File names already warned about, so a broken file is not logged every second.
    warned: Mutex<HashSet<String>>,
}

pub enum Liveness {
    /// pid to lstart, for the pids `ps` knew
    Alive(HashMap<u32, String>),
    Failed,
}

// wired into main in the next task
#[allow(dead_code)]
impl HookSource {
    pub fn new() -> HookSource {
        HookSource::with_dir(sessions_dir())
    }

    #[cfg(test)]
    fn in_dir(dir: PathBuf) -> HookSource {
        HookSource::with_dir(Some(dir))
    }

    fn with_dir(dir: Option<PathBuf>) -> HookSource {
        HookSource {
            dir,
            warned: Mutex::new(HashSet::new()),
        }
    }

    #[cfg(test)]
    fn fetch_at(&self, now_ms: u64) -> Vec<Agent> {
        self.fetch_with(now_ms, run_ps_liveness)
    }

    fn fetch_with(&self, now_ms: u64, liveness: impl FnOnce(&[u32]) -> Liveness) -> Vec<Agent> {
        let Some(dir) = &self.dir else {
            return vec![];
        };
        let records = self.read_records(dir);
        let mut pids: Vec<u32> = records.iter().filter_map(|(_, r)| r.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        let alive = if pids.is_empty() {
            None
        } else {
            match liveness(&pids) {
                Liveness::Alive(map) => Some(map),
                Liveness::Failed => None,
            }
        };
        records
            .into_iter()
            .filter(|(path, r)| {
                let dead = match r.pid {
                    // Without a ps answer nothing owned by a pid can be told dead.
                    Some(pid) => {
                        alive
                            .as_ref()
                            .is_some_and(|map| match (map.get(&pid), &r.pid_start) {
                                (None, _) => true,
                                (Some(start), Some(recorded)) => start != recorded,
                                (Some(_), None) => true,
                            })
                    }
                    None => now_ms.saturating_sub(r.updated_at) > NO_OWNER_TTL_MS,
                };
                !(dead && remove_if_unchanged(path, r.updated_at))
            })
            .map(|(_, r)| r.to_agent())
            .collect()
    }

    fn first_warning(&self, name: &str) -> bool {
        self.warned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name.to_string())
    }

    fn read_records(&self, dir: &Path) -> Vec<(PathBuf, SessionRecord)> {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return vec![];
        };
        let mut records = vec![];
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            let path = entry.path();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| SessionRecord::from_json(&text));
            match parsed {
                Ok(record) => records.push((path, record)),
                Err(e) => {
                    if self.first_warning(&name) {
                        tracing::warn!("skipping session file {name}: {e}");
                    }
                }
            }
        }
        records
    }
}

// wired into main in the next task
#[allow(dead_code)]
impl DataSource for HookSource {
    fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
        Ok(self.fetch_with(now_ms(), run_ps_liveness))
    }

    fn emitted_statuses(&self) -> &'static [Status] {
        EMITTED
    }
}

/// Removes the record only if it is still the one that was judged dead: a hook may have
/// rewritten it since. Returns whether the record is gone. There is no lock, so a write
/// landing between this read and the removal is lost (accepted, spec §6).
fn remove_if_unchanged(path: &Path, updated_at: u64) -> bool {
    let current = match std::fs::read_to_string(path) {
        Ok(text) => SessionRecord::from_json(&text).ok().map(|r| r.updated_at),
        Err(e) => return e.kind() == std::io::ErrorKind::NotFound,
    };
    current == Some(updated_at) && std::fs::remove_file(path).is_ok()
}

fn run_ps_liveness(pids: &[u32]) -> Liveness {
    let list: Vec<String> = pids.iter().map(u32::to_string).collect();
    let out = output_with_timeout(
        Command::new("ps")
            .env("LC_ALL", "C")
            .args(["-o", "pid=,lstart=", "-p", &list.join(",")]),
        PS_TIMEOUT,
    );
    match out {
        Ok(out) => parse_liveness(
            out.status.code().unwrap_or(-1),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(_) => Liveness::Failed,
    }
}

/// `ps -p` exits 0 when it knows at least one pid and 1 with no output when it knows none.
fn parse_liveness(exit_code: i32, stdout: &str, stderr: &str) -> Liveness {
    match exit_code {
        0 => Liveness::Alive(
            stdout
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    let pid = fields.next()?.parse().ok()?;
                    Some((pid, fields.collect::<Vec<_>>().join(" ")))
                })
                .collect(),
        ),
        1 if stdout.trim().is_empty() && stderr.trim().is_empty() => {
            Liveness::Alive(HashMap::new())
        }
        _ => Liveness::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::hook::SessionRecord;
    use agent_core::Status;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;

    /// A temp dir that is removed on drop. `path()` is a child that does not exist yet, so
    /// the tests also cover `write_event` creating the sessions directory.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let root =
                std::env::temp_dir().join(format!("turnray-hooks-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            TempDir(root)
        }

        fn path(&self) -> PathBuf {
            self.0.join("sessions")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn event(name: &str, extra: &str) -> Vec<u8> {
        format!(
            r#"{{"session_id":"abc-123","cwd":"/Users/x/proj","hook_event_name":"{name}"{extra}}}"#
        )
        .into_bytes()
    }

    fn owner() -> Option<(u32, String)> {
        Some((4242, "Thu Oct 29 18:19:06 2026".to_string()))
    }

    fn record(dir: &Path) -> SessionRecord {
        SessionRecord::from_json(&std::fs::read_to_string(dir.join("abc-123.json")).unwrap())
            .unwrap()
    }

    fn entries(dir: &Path) -> Vec<String> {
        match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => vec![],
        }
    }

    const PROMPT: &str = r#","prompt":"Fix the bug""#;

    #[test]
    fn a_prompt_writes_a_working_record_with_title_cwd_time_and_owner() {
        let t = TempDir::new("prompt");
        write_event(
            &t.path(),
            &event("UserPromptSubmit", PROMPT),
            1_000,
            owner(),
        )
        .unwrap();
        assert_eq!(
            record(&t.path()),
            SessionRecord {
                status: Status::Working,
                cwd: "/Users/x/proj".into(),
                title: Some("Fix the bug".into()),
                updated_at: 1_000,
                pid: Some(4242),
                pid_start: Some("Thu Oct 29 18:19:06 2026".into()),
            }
        );
        assert_eq!(entries(&t.path()), vec!["abc-123.json"]);
    }

    #[test]
    fn stop_marks_done_and_keeps_the_title() {
        let t = TempDir::new("stop");
        write_event(
            &t.path(),
            &event("UserPromptSubmit", PROMPT),
            1_000,
            owner(),
        )
        .unwrap();
        write_event(&t.path(), &event("Stop", ""), 2_000, owner()).unwrap();
        let r = record(&t.path());
        assert_eq!(r.status, Status::Done);
        assert_eq!(r.title, Some("Fix the bug".into()));
        assert_eq!(r.updated_at, 2_000);
    }

    #[test]
    fn session_end_removes_the_file_and_is_ok_without_one() {
        let t = TempDir::new("end");
        write_event(
            &t.path(),
            &event("UserPromptSubmit", PROMPT),
            1_000,
            owner(),
        )
        .unwrap();
        write_event(&t.path(), &event("SessionEnd", ""), 2_000, owner()).unwrap();
        assert!(entries(&t.path()).is_empty());
        write_event(&t.path(), &event("SessionEnd", ""), 3_000, owner()).unwrap();
    }

    #[test]
    fn a_write_older_than_the_record_is_skipped() {
        let t = TempDir::new("stale");
        write_event(
            &t.path(),
            &event("UserPromptSubmit", PROMPT),
            5_000,
            owner(),
        )
        .unwrap();
        let before = std::fs::read_to_string(t.path().join("abc-123.json")).unwrap();
        write_event(&t.path(), &event("Stop", ""), 4_999, owner()).unwrap();
        assert_eq!(
            std::fs::read_to_string(t.path().join("abc-123.json")).unwrap(),
            before
        );
        // The same millisecond is not older, so it is written.
        write_event(&t.path(), &event("Stop", ""), 5_000, owner()).unwrap();
        assert_eq!(record(&t.path()).status, Status::Done);
    }

    #[test]
    fn a_session_id_that_escapes_the_directory_is_an_error_and_writes_nothing() {
        let t = TempDir::new("evil");
        let input = br#"{"session_id":"../evil","cwd":"/x","hook_event_name":"Stop"}"#;
        assert!(write_event(&t.path(), input, 1_000, owner()).is_err());
        assert!(entries(&t.path()).is_empty());
        assert!(entries(&t.0).is_empty());
    }

    #[test]
    fn malformed_json_is_an_error_and_writes_nothing() {
        let t = TempDir::new("bad-json");
        assert!(write_event(&t.path(), b"not json", 1_000, owner()).is_err());
        assert!(entries(&t.0).is_empty());
    }

    #[test]
    fn a_subagent_event_creates_no_file() {
        let t = TempDir::new("subagent");
        write_event(
            &t.path(),
            &event("Stop", r#","agent_id":"sub-1""#),
            1_000,
            owner(),
        )
        .unwrap();
        assert!(entries(&t.path()).is_empty());
    }

    #[test]
    fn the_file_is_0600_and_the_created_directory_is_0700() {
        let t = TempDir::new("modes");
        write_event(&t.path(), &event("Stop", ""), 1_000, owner()).unwrap();
        let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(t.path().join("abc-123.json")), 0o600);
        assert_eq!(mode(t.path()), 0o700);
    }

    #[test]
    fn herdr_env_skips_the_write_but_still_drains_the_input() {
        let t = TempDir::new("herdr-env");
        let input = event("Stop", "");
        let mut reader = Cursor::new(input.clone());
        let owner = || -> Option<(u32, String)> { panic!("ps must not run") };
        handle(&t.path(), &mut reader, true, 1_000, owner, 1024).unwrap();
        assert!(entries(&t.path()).is_empty());
        assert_eq!(reader.position() as usize, input.len());
    }

    #[test]
    fn a_no_op_event_does_not_look_up_the_owner() {
        let t = TempDir::new("no-owner");
        let owner = || -> Option<(u32, String)> { panic!("ps must not run") };
        let reader = Cursor::new(event(
            "Notification",
            r#","notification_type":"auth_success""#,
        ));
        handle(&t.path(), reader, false, 1_000, owner, 1024).unwrap();
    }

    #[test]
    fn handle_writes_the_record_for_a_normal_event() {
        let t = TempDir::new("handle");
        handle(
            &t.path(),
            Cursor::new(event("Stop", "")),
            false,
            1_000,
            owner,
            1024,
        )
        .unwrap();
        assert_eq!(record(&t.path()).pid, Some(4242));
    }

    #[test]
    fn read_capped_keeps_exactly_cap_bytes_and_rejects_one_more() {
        assert_eq!(
            read_capped(Cursor::new(vec![7u8; 16]), 16),
            Some(vec![7u8; 16])
        );
        let mut reader = Cursor::new(vec![7u8; 17]);
        assert_eq!(read_capped(&mut reader, 16), None);
        assert_eq!(reader.position(), 17);
        // Larger than the internal read buffer, so one read cannot take it all.
        let len = 64 * 1024 + 1;
        let mut reader = Cursor::new(vec![7u8; len]);
        assert_eq!(read_capped(&mut reader, 16), None);
        assert_eq!(reader.position() as usize, len);
    }

    #[test]
    fn a_newer_record_written_during_the_owner_lookup_is_not_overwritten() {
        let t = TempDir::new("race");
        write_event(
            &t.path(),
            &event("UserPromptSubmit", PROMPT),
            1_000,
            owner(),
        )
        .unwrap();
        let dir = t.path();
        let concurrent_stop = || {
            write_event(&dir, &event("Stop", ""), 9_000, None).unwrap();
            owner()
        };
        write_event_with(&dir, &event("PostToolUse", ""), 2_000, concurrent_stop).unwrap();
        let r = record(&dir);
        assert_eq!((r.status, r.updated_at), (Status::Done, 9_000));
    }

    #[test]
    fn the_existing_status_decides_whether_idle_prompt_applies() {
        let idle_prompt = event("Notification", r#","notification_type":"idle_prompt""#);
        let t = TempDir::new("idle-working");
        write_event(
            &t.path(),
            &event("UserPromptSubmit", PROMPT),
            1_000,
            owner(),
        )
        .unwrap();
        write_event(&t.path(), &idle_prompt, 2_000, owner()).unwrap();
        assert_eq!(record(&t.path()).status, Status::Idle);

        let t = TempDir::new("idle-done");
        write_event(&t.path(), &event("Stop", ""), 1_000, owner()).unwrap();
        write_event(&t.path(), &idle_prompt, 2_000, owner()).unwrap();
        assert_eq!(record(&t.path()).status, Status::Done);
    }

    #[test]
    fn input_over_the_cap_writes_nothing() {
        let t = TempDir::new("over-cap");
        let input = event("Stop", "");
        let cap = input.len() - 1;
        assert!(handle(&t.path(), Cursor::new(input), false, 1_000, owner, cap).is_err());
        assert!(entries(&t.path()).is_empty());
    }

    /// A process table of (pid, ppid, comm); lstart is derived from the pid. Every lookup
    /// is recorded in `asked`.
    fn walk(
        start: u32,
        table: &[(u32, u32, &str)],
    ) -> (Option<(u32, String)>, std::cell::RefCell<Vec<u32>>) {
        let asked = std::cell::RefCell::new(Vec::new());
        let found = find_claude_from(start, |pid| {
            asked.borrow_mut().push(pid);
            table
                .iter()
                .find(|(p, _, _)| *p == pid)
                .map(|(p, ppid, comm)| (*ppid, comm.to_string(), format!("lstart-{p}")))
        });
        (found, asked)
    }

    #[test]
    fn find_claude_from_finds_claude_behind_the_plugins_sh() {
        let table = [(300, 200, "sh"), (200, 100, "claude"), (100, 1, "zsh")];
        let (found, asked) = walk(300, &table);
        assert_eq!(found, Some((200, "lstart-200".to_string())));
        assert_eq!(*asked.borrow(), vec![300, 200]);
    }

    #[test]
    fn find_claude_from_finds_claude_as_the_direct_parent() {
        let (found, asked) = walk(200, &[(200, 100, "claude")]);
        assert_eq!(found, Some((200, "lstart-200".to_string())));
        assert_eq!(*asked.borrow(), vec![200]);
    }

    #[test]
    fn find_claude_from_matches_the_basename_of_a_full_path() {
        let (found, _) = walk(200, &[(200, 100, "/opt/homebrew/bin/claude")]);
        assert_eq!(found, Some((200, "lstart-200".to_string())));
    }

    #[test]
    fn find_claude_from_gives_up_after_the_ancestor_depth() {
        let table = [
            (4, 3, "sh"),
            (3, 2, "zsh"),
            (2, 1, "tmux"),
            (1, 0, "claude"),
        ];
        let (found, asked) = walk(4, &table);
        assert_eq!(found, None);
        assert_eq!(*asked.borrow(), vec![4, 3, 2]);
    }

    #[test]
    fn find_claude_from_returns_none_when_a_lookup_fails_midway() {
        // 100 is missing from the table, though claude sits above it.
        let table = [(300, 200, "sh"), (200, 100, "zsh"), (99, 1, "claude")];
        let (found, asked) = walk(300, &table);
        assert_eq!(found, None);
        assert_eq!(*asked.borrow(), vec![300, 200, 100]);
    }

    #[test]
    fn parse_ps_ancestor_splits_ppid_comm_and_lstart() {
        assert_eq!(
            parse_ps_ancestor("  123 /usr/local/bin/claude Thu Oct 29 18:19:06 2026"),
            Some((
                123,
                "/usr/local/bin/claude".into(),
                "Thu Oct 29 18:19:06 2026".into()
            ))
        );
        assert_eq!(parse_ps_ancestor(""), None);
        assert_eq!(parse_ps_ancestor("   \n"), None);
    }

    fn put(dir: &Path, name: &str, r: &SessionRecord) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), r.to_json()).unwrap();
    }

    fn rec(updated_at: u64, pid: Option<u32>, pid_start: Option<&str>) -> SessionRecord {
        SessionRecord {
            status: Status::Working,
            cwd: "/Users/x/proj".into(),
            title: None,
            updated_at,
            pid,
            pid_start: pid_start.map(Into::into),
        }
    }

    fn source_with(t: &TempDir, records: &[(&str, SessionRecord)]) -> HookSource {
        for (name, r) in records {
            put(&t.path(), name, r);
        }
        HookSource::in_dir(t.path())
    }

    fn never_called(_: &[u32]) -> Liveness {
        panic!("ps must not run")
    }

    #[test]
    fn a_missing_directory_gives_no_agents() {
        let t = TempDir::new("read-missing");
        assert!(HookSource::in_dir(t.path())
            .fetch_with(0, never_called)
            .is_empty());
    }

    #[test]
    fn a_record_of_a_live_process_is_listed_and_kept() {
        let t = TempDir::new("read-live");
        let pid = std::process::id();
        let start = ps_lstart(pid);
        let record = SessionRecord {
            title: Some("Fix the bug".into()),
            ..rec(1_000, Some(pid), Some(&start))
        };
        let s = source_with(&t, &[("a.json", record)]);
        assert_eq!(
            s.fetch_at(2_000),
            vec![Agent {
                status: Status::Working,
                name: "claude".into(),
                workspace: "proj".into(),
                title: "Fix the bug".into(),
            }]
        );
        assert_eq!(entries(&t.path()), vec!["a.json"]);
    }

    #[test]
    fn a_record_of_a_pid_that_ps_does_not_know_is_removed() {
        let t = TempDir::new("read-gone");
        let s = source_with(
            &t,
            &[(
                "a.json",
                rec(1_000, Some(4242), Some("Thu Oct 29 18:19:06 2026")),
            )],
        );
        assert!(s
            .fetch_with(2_000, |_| Liveness::Alive(HashMap::new()))
            .is_empty());
        assert!(entries(&t.path()).is_empty());
    }

    #[test]
    fn a_record_with_a_pid_but_no_recorded_start_is_removed() {
        let t = TempDir::new("read-no-start");
        let s = source_with(&t, &[("a.json", rec(1_000, Some(4242), None))]);
        let alive = HashMap::from([(4242, "Thu Oct 29 18:19:06 2026".to_string())]);
        assert!(s.fetch_with(2_000, |_| Liveness::Alive(alive)).is_empty());
        assert!(entries(&t.path()).is_empty());
    }

    #[test]
    fn a_record_whose_pid_was_reused_is_removed() {
        let t = TempDir::new("read-reused");
        let pid = std::process::id();
        let s = source_with(
            &t,
            &[(
                "a.json",
                rec(1_000, Some(pid), Some("Thu Oct 29 18:19:06 1999")),
            )],
        );
        assert!(s.fetch_at(2_000).is_empty());
        assert!(entries(&t.path()).is_empty());
    }

    #[test]
    fn a_record_without_a_pid_expires_after_24_hours_exactly() {
        let t = TempDir::new("read-ttl");
        let day = 24 * 60 * 60 * 1000;
        let s = source_with(
            &t,
            &[
                ("edge.json", rec(1_000, None, None)),
                ("over.json", rec(999, None, None)),
                ("inside.json", rec(1_001, None, None)),
            ],
        );
        assert_eq!(s.fetch_with(1_000 + day, never_called).len(), 2);
        let mut left = entries(&t.path());
        left.sort();
        assert_eq!(left, vec!["edge.json", "inside.json"]);
    }

    #[test]
    fn a_broken_file_is_skipped_and_left_alone() {
        let t = TempDir::new("read-broken");
        std::fs::create_dir_all(t.path()).unwrap();
        std::fs::write(t.path().join("bad.json"), "not json").unwrap();
        let s = HookSource::in_dir(t.path());
        assert!(s.fetch_with(0, never_called).is_empty());
        assert!(t.path().join("bad.json").exists());
    }

    #[test]
    fn a_hidden_temp_file_is_not_read_even_when_it_holds_a_valid_record() {
        let t = TempDir::new("read-hidden");
        let s = source_with(
            &t,
            &[
                (".abc.1.tmp.json", rec(1_000, None, None)),
                ("note.txt", rec(1_000, None, None)),
            ],
        );
        assert!(s.fetch_with(2_000, never_called).is_empty());
    }

    #[test]
    fn a_broken_file_is_warned_about_on_the_first_fetch_only() {
        let t = TempDir::new("read-warn-once");
        std::fs::create_dir_all(t.path()).unwrap();
        std::fs::write(t.path().join("bad.json"), "not json").unwrap();
        let s = HookSource::in_dir(t.path());
        // Nothing was warned about yet, so the first claim of the name succeeds.
        assert!(s.first_warning("other.json"));
        assert!(!s.first_warning("other.json"));
        s.fetch_with(0, never_called);
        // The fetch claimed "bad.json" itself, so a later fetch stays quiet.
        assert!(!s.first_warning("bad.json"));
    }

    #[test]
    fn a_failed_ps_keeps_pid_records_but_expired_pidless_ones_still_go() {
        let t = TempDir::new("read-ps-failed");
        let s = source_with(
            &t,
            &[
                ("pid.json", rec(0, Some(99_990), Some("x"))),
                ("old.json", rec(0, None, None)),
            ],
        );
        let now = 24 * 60 * 60 * 1000 + 1;
        let agents = s.fetch_with(now, |_| Liveness::Failed);
        assert_eq!(agents.len(), 1);
        assert_eq!(entries(&t.path()), vec!["pid.json"]);
    }

    #[test]
    fn ps_is_asked_once_for_every_pid_and_not_at_all_without_pids() {
        let t = TempDir::new("read-ps-once");
        let s = source_with(
            &t,
            &[
                ("a.json", rec(1, Some(11), Some("x"))),
                ("b.json", rec(1, Some(22), Some("y"))),
                ("c.json", rec(1, None, None)),
            ],
        );
        let asked = std::cell::RefCell::new(vec![]);
        s.fetch_with(2, |pids| {
            asked.borrow_mut().push(pids.to_vec());
            Liveness::Failed
        });
        let mut asked = asked.into_inner();
        assert_eq!(asked.len(), 1);
        asked[0].sort();
        assert_eq!(asked[0], vec![11, 22]);
    }

    #[test]
    fn a_record_rewritten_before_the_removal_is_not_removed() {
        let t = TempDir::new("read-reread");
        put(&t.path(), "a.json", &rec(5_000, None, None));
        let path = t.path().join("a.json");
        assert!(!remove_if_unchanged(&path, 1_000));
        assert!(path.exists());
        assert!(remove_if_unchanged(&path, 5_000));
        assert!(!path.exists());
        // Already gone, as when the hook's SessionEnd got there first.
        assert!(remove_if_unchanged(&path, 5_000));
    }

    #[test]
    fn parse_liveness_reads_pid_and_lstart_per_line() {
        let Liveness::Alive(map) = parse_liveness(0, "  123 Thu Oct 29 18:19:06 2026\n", "") else {
            panic!("expected Alive");
        };
        assert_eq!(
            map.get(&123).map(String::as_str),
            Some("Thu Oct 29 18:19:06 2026")
        );
    }

    #[test]
    fn parse_liveness_reads_every_line() {
        let out = "  11 Thu Oct 29 18:19:06 2026\n 2222 Fri Oct 30 01:02:03 2026\n";
        let Liveness::Alive(map) = parse_liveness(0, out, "") else {
            panic!("expected Alive");
        };
        assert_eq!(map.len(), 2);
        assert_eq!(map[&11], "Thu Oct 29 18:19:06 2026");
        assert_eq!(map[&2222], "Fri Oct 30 01:02:03 2026");
    }

    #[test]
    fn parse_liveness_treats_exit_1_with_no_output_as_nobody_alive() {
        assert!(matches!(parse_liveness(1, "", ""), Liveness::Alive(m) if m.is_empty()));
        assert!(matches!(parse_liveness(1, " \n", "\n"), Liveness::Alive(m) if m.is_empty()));
    }

    #[test]
    fn parse_liveness_fails_on_any_other_error() {
        assert!(matches!(
            parse_liveness(1, "", "ps: Invalid process id: abc"),
            Liveness::Failed
        ));
        assert!(matches!(parse_liveness(2, "", ""), Liveness::Failed));
        assert!(matches!(parse_liveness(-1, "", ""), Liveness::Failed));
    }

    /// The real `ps` answer for a pid, the same call the source makes.
    fn ps_lstart(pid: u32) -> String {
        match run_ps_liveness(&[pid]) {
            Liveness::Alive(mut m) => m.remove(&pid).expect("pid is alive"),
            Liveness::Failed => panic!("ps failed"),
        }
    }
}
