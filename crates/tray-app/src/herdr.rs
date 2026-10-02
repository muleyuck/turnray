//! The herdr data source: runs `herdr agent list` / `herdr workspace list`. herdr answers
//! these over its socket, so they work from outside herdr with no `HERDR_*` variables.

use std::path::{Path, PathBuf};
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

fn resolve_from(candidates: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates.iter().find(|p| exists(p.as_path())).cloned()
}

/// `command -v` because zsh's `which` is a builtin that varies by setup.
fn resolve_via_login_shell() -> Option<PathBuf> {
    let out = std::process::Command::new("/bin/zsh")
        .arg("-lc")
        .arg("command -v herdr")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
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
    let out = match std::process::Command::new(path).args(args).output() {
        Ok(out) => out,
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

pub struct HerdrSource;

impl DataSource for HerdrSource {
    fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
        let path = resolve().ok_or_else(not_found)?;
        let agents = run(&path, &["agent", "list"])?;
        // Labels only decorate the list; without them each line shows the workspace id.
        let labels = match run(&path, &["workspace", "list"])
            .and_then(|json| herdr::parse_workspaces(&json).map_err(SourceError::Failed))
        {
            Ok(labels) => labels,
            Err(e) => {
                tracing::warn!("workspace labels unavailable: {e:?}");
                Default::default()
            }
        };
        herdr::parse_agents(&agents, &labels).map_err(SourceError::Failed)
    }

    fn emitted_statuses(&self) -> &'static [Status] {
        herdr::EMITTED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

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
