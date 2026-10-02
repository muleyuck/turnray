//! The herdr data source: runs `herdr agent list` / `herdr workspace list`. herdr answers
//! these over its socket, so they work from outside herdr with no `HERDR_*` variables.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use agent_core::{herdr, Agent, DataSource, SourceError, Status};

/// `~/.local/bin/herdr` and `~/.cargo/bin/herdr` belong here too, but need `$HOME`, so
/// `candidates` adds them.
const KNOWN_PATHS: &[&str] = &[
    "/opt/homebrew/bin/herdr",
    "/usr/local/bin/herdr",
    "/opt/local/bin/herdr",
    "/usr/bin/herdr",
];

/// The poll runs every second, so a miss must not spawn a login shell each time; still
/// short enough that an install takes effect while the app runs.
const SHELL_MISS_TTL: Duration = Duration::from_secs(600);

/// A hung herdr socket or a shell startup file waiting for input would otherwise stall
/// the poll for good, leaving the last statuses on show as if they were current.
const HERDR_TIMEOUT: Duration = Duration::from_secs(5);
const SHELL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait for the drain threads once the child has exited.
const EXIT_GRACE: Duration = Duration::from_millis(200);

type ShellCache = std::sync::Mutex<Option<(Option<PathBuf>, Instant)>>;

/// What the login-shell fallback found, and when. Under launchd the `.app` has no shell
/// ancestry and PATH is the bare `/usr/bin:/bin:/usr/sbin:/sbin`, so the shell is the
/// expensive last resort: a hit is kept until `invalidate()`, a miss until
/// `SHELL_MISS_TTL`. The known-path probe ahead of it is never cached.
static SHELL_RESOLVED: ShellCache = std::sync::Mutex::new(None);

fn candidates() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = KNOWN_PATHS.iter().map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".local/bin/herdr"));
        paths.push(home.join(".cargo/bin/herdr"));
    }
    paths
}

/// `Command::output` with a deadline: past it the child's whole process group is killed
/// and `TimedOut` returned. Its pipes are drained on their own threads so a chatty child
/// can't block on a full pipe.
fn output_with_timeout(cmd: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group, so a timeout also takes down whatever it started (a login shell's
        // startup files, say) instead of leaving it behind.
        .process_group(0)
        .spawn()?;
    let stdout = Drain::spawn(child.stdout.take());
    let stderr = Drain::spawn(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_group(child.id());
                let _ = child.wait();
                return Err(e);
            }
        }
        if Instant::now() >= deadline {
            kill_group(child.id());
            let _ = child.wait();
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // The child has exited, so its output is already in the pipe. Something it left running
    // in the background may hold the pipe open past that, so the wait for the end of the
    // pipe is short and what was read by then is the answer.
    let until = Instant::now() + EXIT_GRACE;
    Ok(Output {
        status,
        stdout: stdout.collect(until),
        stderr: stderr.collect(until),
    })
}

/// Reads a pipe to its end on a thread, keeping what it has read so far reachable.
struct Drain {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
}

impl Drain {
    fn spawn(pipe: Option<impl Read + Send + 'static>) -> Drain {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, done) = std::sync::mpsc::channel();
        let shared = std::sync::Arc::clone(&buf);
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0; 4096];
                while let Ok(n @ 1..) = pipe.read(&mut chunk) {
                    shared
                        .lock()
                        .expect("drain buffer")
                        .extend_from_slice(&chunk[..n]);
                }
            }
            let _ = tx.send(());
        });
        Drain { buf, done }
    }

    /// Waits until `until` at most for the end of the pipe, then returns what has arrived.
    fn collect(self, until: Instant) -> Vec<u8> {
        let _ = self
            .done
            .recv_timeout(until.saturating_duration_since(Instant::now()));
        std::mem::take(&mut *self.buf.lock().expect("drain buffer"))
    }
}

/// std can only signal the child itself, so the group goes through kill(1).
fn kill_group(pgid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pgid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn resolve_from(candidates: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates.iter().find(|p| exists(p.as_path())).cloned()
}

/// `command -v` because zsh's `which` is a builtin that varies by setup.
fn resolve_via_login_shell() -> Option<PathBuf> {
    let out = output_with_timeout(
        Command::new("/bin/zsh").arg("-lc").arg("command -v herdr"),
        SHELL_TIMEOUT,
    )
    .ok()?;
    if !out.status.success() {
        return None;
    }
    path_from_shell_output(&String::from_utf8_lossy(&out.stdout))
}

/// Login-shell startup and logout files may print to stdout too, so the answer is the last
/// line that is an absolute path to a `herdr`. An alias or function answers with neither.
fn path_from_shell_output(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .map(str::trim)
        .map(Path::new)
        .rfind(|p| p.is_absolute() && p.file_name() == Some("herdr".as_ref()))
        .map(Path::to_path_buf)
}

/// Filesystem, shell and clock are injected so a test can drive the caching.
fn resolve_with(
    candidates: &[PathBuf],
    cache: &ShellCache,
    now: Instant,
    exists: impl Fn(&Path) -> bool,
    shell: impl Fn() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(found) = resolve_from(candidates, exists) {
        return Some(found);
    }
    if let Some((cached, at)) = cache.lock().expect("herdr path cache").as_ref() {
        if cached.is_some() || now.duration_since(*at) < SHELL_MISS_TTL {
            return cached.clone();
        }
    }
    // The lock is released above, so it is not held across the shell call.
    let found = shell();
    *cache.lock().expect("herdr path cache") = Some((found.clone(), now));
    found
}

fn resolve() -> Option<PathBuf> {
    resolve_with(
        &candidates(),
        &SHELL_RESOLVED,
        Instant::now(),
        |p| p.exists(),
        resolve_via_login_shell,
    )
}

/// Called when a resolved path has stopped working, i.e. herdr was uninstalled while we ran.
fn invalidate() {
    *SHELL_RESOLVED.lock().expect("herdr path cache") = None;
}

fn not_found() -> SourceError {
    SourceError::NotFound("herdr not found".to_string())
}

/// herdr reports a failure as a nonzero exit with the reason on stderr.
fn classify(status_code: i32, stdout: &str, stderr: &str) -> Result<String, SourceError> {
    if status_code == 0 {
        return Ok(stdout.to_string());
    }
    let reason = stderr.lines().find(|l| !l.trim().is_empty());
    Err(SourceError::Failed(match reason {
        Some(line) => format!("herdr not reachable: {}", line.trim()),
        None => format!("herdr not reachable (exit {status_code})"),
    }))
}

/// Blocking: it starts a process.
fn run(path: &Path, args: &[&str]) -> Result<String, SourceError> {
    let out = match output_with_timeout(Command::new(path).args(args), HERDR_TIMEOUT) {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            return Err(SourceError::Failed(format!(
                "herdr did not answer within {}s",
                HERDR_TIMEOUT.as_secs()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            invalidate();
            return Err(not_found());
        }
        Err(e) => return Err(SourceError::Failed(format!("herdr failed to start: {e}"))),
    };
    classify(
        out.status.code().unwrap_or(-1),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

#[derive(Default)]
pub struct HerdrSource {
    /// The last label failure logged, so a persistent one isn't logged every poll.
    last_label_error: std::sync::Mutex<Option<String>>,
}

impl DataSource for HerdrSource {
    fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
        self.fetch_from(&resolve().ok_or_else(not_found)?)
    }

    fn emitted_statuses(&self) -> &'static [Status] {
        herdr::EMITTED
    }
}

impl HerdrSource {
    /// `fetch` with the executable already resolved, so a test can point it at a fake.
    fn fetch_from(&self, path: &Path) -> Result<Vec<Agent>, SourceError> {
        let agents = run(path, &["agent", "list"])?;
        // Labels only decorate the list; without them each line shows the workspace id.
        let labels = match run(path, &["workspace", "list"])
            .and_then(|json| herdr::parse_workspaces(&json).map_err(SourceError::Failed))
        {
            Ok(labels) => {
                *self.last_label_error.lock().expect("label error") = None;
                labels
            }
            Err(e) => {
                let msg = format!("{e:?}");
                let mut last = self.last_label_error.lock().expect("label error");
                if last.as_deref() != Some(msg.as_str()) {
                    tracing::warn!("workspace labels unavailable: {msg}");
                    *last = Some(msg);
                }
                Default::default()
            }
        };
        herdr::parse_agents(&agents, &labels).map_err(SourceError::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- fetch against a fake herdr ---

    /// An executable shell script standing in for herdr. It answers only the exact
    /// subcommands given, so a wrong argument fails the test instead of passing silently.
    struct FakeHerdr {
        dir: PathBuf,
    }

    impl FakeHerdr {
        fn new(name: &str, agent_list: &str, workspace_list: &str) -> FakeHerdr {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir()
                .join(format!("turnray-fake-herdr-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let script = format!(
                "#!/bin/sh\ncase \"$*\" in\n\"agent list\")\n{agent_list}\n;;\n\"workspace list\")\n{workspace_list}\n;;\n*) echo \"unexpected args: $*\" >&2; exit 64 ;;\nesac\n"
            );
            let path = dir.join("herdr");
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            FakeHerdr { dir }
        }

        fn path(&self) -> PathBuf {
            self.dir.join("herdr")
        }
    }

    impl Drop for FakeHerdr {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A shell command printing `json`, quoted for the fake's case arms.
    fn prints(json: &str) -> String {
        format!("cat <<'EOF'\n{json}\nEOF")
    }

    const AGENT_LIST: &str = r#"{"id":"cli:agent:list","result":{"agents":[
        {"agent":"claude","agent_status":"blocked","focused":false,"pane_id":"w1:p1",
         "terminal_title_stripped":"Fix & ship","workspace_id":"w1"},
        {"agent":"codex","agent_status":"working","focused":true,"pane_id":"w2:p1",
         "terminal_title_stripped":"Refactor","workspace_id":"w2"},
        {"agent":"claude","agent_status":"idle","pane_id":"","workspace_id":"w1"}
        ],"type":"agent_list"}}"#;
    const WORKSPACE_LIST: &str = r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[
        {"label":"turnray","workspace_id":"w1"},{"label":"scoptray","workspace_id":"w2"}]}}"#;

    #[test]
    fn fetch_runs_both_subcommands_and_joins_the_workspace_labels() {
        let fake = FakeHerdr::new("ok", &prints(AGENT_LIST), &prints(WORKSPACE_LIST));
        let agents = HerdrSource::default().fetch_from(&fake.path()).unwrap();
        assert_eq!(
            agents,
            vec![
                Agent {
                    status: Status::Blocked,
                    name: "claude".into(),
                    workspace: "turnray".into(),
                    title: "Fix & ship".into(),
                    focused: false,
                },
                Agent {
                    status: Status::Working,
                    name: "codex".into(),
                    workspace: "scoptray".into(),
                    title: "Refactor".into(),
                    focused: true,
                },
            ]
        );
    }

    #[test]
    fn a_failing_agent_list_is_reported_with_herdrs_reason() {
        let fake = FakeHerdr::new(
            "agent-fails",
            "echo 'Error: server not running' >&2; exit 1",
            &prints(WORKSPACE_LIST),
        );
        assert_eq!(
            HerdrSource::default().fetch_from(&fake.path()),
            Err(SourceError::Failed(
                "herdr not reachable: Error: server not running".into()
            ))
        );
    }

    #[test]
    fn a_failing_workspace_list_falls_back_to_workspace_ids() {
        let fake = FakeHerdr::new(
            "workspace-fails",
            &prints(AGENT_LIST),
            "echo 'Error: boom' >&2; exit 1",
        );
        let agents = HerdrSource::default().fetch_from(&fake.path()).unwrap();
        let workspaces: Vec<&str> = agents.iter().map(|a| a.workspace.as_str()).collect();
        assert_eq!(workspaces, ["w1", "w2"]);
    }

    #[test]
    fn an_agent_list_that_is_not_json_is_a_failure() {
        let fake = FakeHerdr::new(
            "not-json",
            "echo 'herdr 9.9 — usage: ...'",
            &prints(WORKSPACE_LIST),
        );
        let Err(SourceError::Failed(msg)) = HerdrSource::default().fetch_from(&fake.path()) else {
            panic!("expected a failure");
        };
        assert!(msg.starts_with("unreadable agent list"), "{msg}");
    }

    #[test]
    fn a_herdr_that_vanished_is_not_found() {
        let fake = FakeHerdr::new("vanished", "", "");
        let path = fake.path();
        drop(fake);
        assert!(matches!(
            HerdrSource::default().fetch_from(&path),
            Err(SourceError::NotFound(_))
        ));
    }

    // --- process handling ---
    use std::cell::{Cell, RefCell};

    #[test]
    fn a_child_that_finishes_in_time_yields_its_output() {
        let out = output_with_timeout(
            Command::new("/bin/sh").args(["-c", "echo out; echo err >&2; exit 3"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
    }

    #[test]
    fn a_child_past_the_deadline_is_killed_and_reported() {
        let started = Instant::now();
        let err = output_with_timeout(
            Command::new("/bin/sleep").arg("10"),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must not wait for the child"
        );
    }

    #[test]
    fn a_background_process_holding_the_pipe_does_not_cost_the_output() {
        // e.g. a ~/.zprofile that starts a job: the answer is written before the shell
        // exits and must not be thrown away for the job still holding stdout.
        let started = Instant::now();
        let out = output_with_timeout(
            Command::new("/bin/sh").args(["-c", "echo /opt/x/herdr; sleep 10 & exit 0"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"/opt/x/herdr\n");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_timeout_also_kills_what_the_child_started() {
        let pid_file = std::env::temp_dir().join(format!("turnray-test-{}", std::process::id()));
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let err = output_with_timeout(
            Command::new("/bin/sh").args(["-c", &script]),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let _ = std::fs::remove_file(&pid_file);
        let alive = || {
            Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let gone_by = Instant::now() + Duration::from_secs(2);
        while alive() && Instant::now() < gone_by {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(), "the background sleep outlived the timeout");
    }

    #[test]
    fn the_shell_answer_is_the_last_absolute_path_to_a_herdr() {
        assert_eq!(
            path_from_shell_output("Welcome /etc/motd\n/opt/x/herdr\n\n"),
            Some(PathBuf::from("/opt/x/herdr"))
        );
        // Startup or logout files may print after it, paths included.
        assert_eq!(
            path_from_shell_output("/opt/x/herdr\n/usr/local/share/bye\nbye\n"),
            Some(PathBuf::from("/opt/x/herdr"))
        );
        // An alias or function answers with something that is not a path to herdr.
        assert_eq!(
            path_from_shell_output("/usr/local/share/banner\nalias herdr='h'\n"),
            None
        );
    }

    #[test]
    fn a_zero_exit_yields_stdout() {
        assert_eq!(classify(0, "{}", ""), Ok("{}".to_string()));
    }

    #[test]
    fn a_nonzero_exit_reports_the_first_stderr_line() {
        // Real output when the socket can't be reached, exit 1
        let stderr =
            "Error: Os { code: 1, kind: PermissionDenied, message: \"Operation not permitted\" }\n";
        assert_eq!(
            classify(1, "", stderr),
            Err(SourceError::Failed(
                "herdr not reachable: Error: Os { code: 1, kind: PermissionDenied, message: \"Operation not permitted\" }".to_string()
            ))
        );
    }

    #[test]
    fn a_silent_failure_still_says_how_it_exited() {
        assert_eq!(
            classify(2, "", "\n"),
            Err(SourceError::Failed(
                "herdr not reachable (exit 2)".to_string()
            ))
        );
    }

    #[test]
    fn resolve_from_stops_at_the_first_existing_candidate() {
        let probed = RefCell::new(Vec::new());
        let candidates = [
            PathBuf::from("/a/herdr"),
            PathBuf::from("/b/herdr"),
            PathBuf::from("/c/herdr"),
        ];
        let found = resolve_from(&candidates, |p| {
            probed.borrow_mut().push(p.to_path_buf());
            p == Path::new("/b/herdr")
        });
        assert_eq!(found, Some(PathBuf::from("/b/herdr")));
        assert_eq!(probed.borrow().len(), 2, "must not probe past a hit");
    }

    #[test]
    fn a_newly_installed_herdr_is_found_without_a_restart() {
        let cache: ShellCache = std::sync::Mutex::new(None);
        let candidates = [PathBuf::from("/opt/homebrew/bin/herdr")];
        let installed = Cell::new(false);
        let now = Instant::now();
        let shell = || None;

        assert_eq!(
            resolve_with(&candidates, &cache, now, |_| installed.get(), shell),
            None
        );
        installed.set(true);
        assert_eq!(
            resolve_with(&candidates, &cache, now, |_| installed.get(), shell),
            Some(PathBuf::from("/opt/homebrew/bin/herdr"))
        );
    }

    #[test]
    fn a_miss_runs_the_login_shell_once_per_ttl() {
        let cache: ShellCache = std::sync::Mutex::new(None);
        let candidates = [PathBuf::from("/a/herdr")];
        let calls = Cell::new(0);
        let shell = || {
            calls.set(calls.get() + 1);
            None
        };
        let t0 = Instant::now();

        for s in 0..3 {
            resolve_with(
                &candidates,
                &cache,
                t0 + Duration::from_secs(s),
                |_| false,
                shell,
            );
        }
        assert_eq!(calls.get(), 1, "still fresh, must not pay for a shell");

        resolve_with(&candidates, &cache, t0 + SHELL_MISS_TTL, |_| false, shell);
        assert_eq!(calls.get(), 2, "expired, must look again");
    }

    #[test]
    fn a_shell_resolved_path_never_expires() {
        let cache: ShellCache = std::sync::Mutex::new(None);
        let candidates = [PathBuf::from("/a/herdr")];
        let calls = Cell::new(0);
        let shell = || {
            calls.set(calls.get() + 1);
            Some(PathBuf::from("/opt/local/bin/herdr"))
        };
        let t0 = Instant::now();

        let first = resolve_with(&candidates, &cache, t0, |_| false, shell);
        let later = resolve_with(
            &candidates,
            &cache,
            t0 + SHELL_MISS_TTL * 10,
            |_| false,
            shell,
        );
        assert_eq!(first, Some(PathBuf::from("/opt/local/bin/herdr")));
        assert_eq!(later, first);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn homebrew_and_per_user_installs_are_candidates() {
        let paths = candidates();
        assert!(paths.contains(&PathBuf::from("/opt/homebrew/bin/herdr")));
        if let Some(home) = std::env::var_os("HOME") {
            assert!(paths.contains(&PathBuf::from(&home).join(".local/bin/herdr")));
            assert!(paths.contains(&PathBuf::from(&home).join(".cargo/bin/herdr")));
        }
    }
}
