//! The Rex data source: runs turnray's Lua script through `rex do`, which reads the OSC
//! 7501 records of every terminal block in every session over one connection.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use agent_core::{program_status, rex, Agent, DataSource, SourceError, Status};

use crate::process::output_with_timeout;

/// A Rex server that hangs would otherwise stall every poll behind it.
const REX_TIMEOUT: Duration = Duration::from_secs(5);

/// Walks every session and returns the terminal blocks that have records.
const SCRIPT: &str = include_str!("rex.lua");

/// Where Rex keeps its `rex`: the server it installs and updates, under `$HOME`, then
/// the one inside the app. A `.app` has no PATH to search.
const APP_REX: &str =
    "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex";

fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(
            "Library/Application Support/com.superlogical.rex/RexServer/Versions/Current/Rex Server.app/Contents/MacOS/rex",
        ));
    }
    paths.push(PathBuf::from(APP_REX));
    paths
}

/// How rex (0.1.0-preview.21) exits when no server is running and autostart is off.
const NO_SERVER_EXIT: i32 = 4;

/// Why a run gave no agents, for the log.
#[derive(Debug, PartialEq)]
struct Failure {
    reason: String,
    /// No Rex server is running: routine, so it is logged at debug, not as a warning.
    no_server: bool,
}

impl From<String> for Failure {
    fn from(reason: String) -> Failure {
        Failure {
            reason,
            no_server: false,
        }
    }
}

/// The Rex app's preferences domain, and the key under which it records which program
/// statuses it has shown the user. Neither is documented: they were read from Rex
/// 0.1.0-preview.35, whose server API has no read state.
const REX_APP_DOMAIN: &str = "com.superlogical.rex";
const SEEN_KEY: &str = "Workspace.programActivitySeen";

/// What the Rex app has shown the user, from its `receipts`: one `{block, record, date}`
/// per status seen, `date` being that status's `updated_at`. Unreadable means nothing seen.
#[cfg(target_os = "macos")]
fn seen_in_rex_app() -> rex::Seen {
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSArray, NSDate, NSDictionary, NSString, NSUserDefaults};

    let mut seen = rex::Seen::default();
    let key = |k: &str| NSString::from_str(k);
    let Some(defaults) =
        NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&key(REX_APP_DOMAIN)))
    else {
        return seen;
    };
    let Some(receipts) = defaults
        .dictionaryForKey(&key(SEEN_KEY))
        .and_then(|all| all.objectForKey(&key("receipts")))
    else {
        return seen;
    };
    let Some(receipts) = receipts.downcast_ref::<NSArray>() else {
        return seen;
    };
    for receipt in receipts.iter() {
        let Some(receipt) = receipt.downcast_ref::<NSDictionary>() else {
            continue;
        };
        let field = |k: &str| receipt.objectForKey(&*key(k));
        let text =
            |k: &str| field(k).and_then(|o| o.downcast_ref::<NSString>().map(|s| s.to_string()));
        let (Some(block), Some(date)) = (text("block"), field("date")) else {
            continue;
        };
        let Some(date) = date.downcast_ref::<NSDate>() else {
            continue;
        };
        let millis = (date.timeIntervalSince1970() * 1000.0).round() as i64;
        seen.insert(&block, &text("record").unwrap_or_default(), millis);
    }
    seen
}

#[cfg(not(target_os = "macos"))]
fn seen_in_rex_app() -> rex::Seen {
    rex::Seen::default()
}

/// Runs the script once.
fn ask(rex: &Path, timeout: Duration, seen: &rex::Seen) -> Result<Vec<Agent>, Failure> {
    let mut cmd = Command::new(rex);
    // Without --autostart=false, rex starts a server when none is running.
    cmd.args(["--autostart=false", "do", "-e", SCRIPT, "--json"]);
    let out = output_with_timeout(&mut cmd, timeout).map_err(|e| match e.kind() {
        std::io::ErrorKind::TimedOut => {
            format!("rex did not answer within {}ms", timeout.as_millis())
        }
        std::io::ErrorKind::FileTooLarge => "rex answered with too much output".to_string(),
        _ => format!("rex failed to start: {e}"),
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason = stderr.lines().find(|l| !l.trim().is_empty());
        return Err(Failure {
            reason: match (reason, out.status.code()) {
                (Some(line), _) => format!("rex not reachable: {}", line.trim()),
                (None, Some(code)) => format!("rex not reachable (exit {code})"),
                (None, None) => "rex not reachable".to_string(),
            },
            no_server: out.status.code() == Some(NO_SERVER_EXIT),
        });
    }
    Ok(rex::parse(&String::from_utf8_lossy(&out.stdout), seen)?)
}

#[derive(Default)]
pub struct RexSource {
    /// The last failure logged, so a persistent one isn't logged every poll.
    last_error: Mutex<Option<String>>,
}

impl RexSource {
    pub fn new() -> RexSource {
        RexSource::default()
    }

    /// `fetch` with the `rex`, the limit and what the user has seen given, so a test can
    /// drive it.
    fn fetch_from(&self, rex: &Path, timeout: Duration, seen: &rex::Seen) -> Vec<Agent> {
        if !rex.exists() {
            return Vec::new();
        }
        let mut last = self.last_error.lock().expect("rex error");
        match ask(rex, timeout, seen) {
            Ok(found) => {
                *last = None;
                found
            }
            Err(Failure { reason, no_server }) => {
                if last.as_ref() != Some(&reason) {
                    if no_server {
                        tracing::debug!("rex: {reason}");
                    } else {
                        tracing::warn!("rex: {reason}");
                    }
                    *last = Some(reason);
                }
                Vec::new()
            }
        }
    }
}

impl DataSource for RexSource {
    /// Never fails: no `rex`, no running server or no answer adds nothing.
    fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
        let Some(rex) = candidates().into_iter().find(|p| p.exists()) else {
            return Ok(Vec::new());
        };
        Ok(self.fetch_from(&rex, REX_TIMEOUT, &seen_in_rex_app()))
    }

    fn emitted_statuses(&self) -> &'static [Status] {
        program_status::EMITTED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    /// A temp dir, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("tr-rex-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A shell script standing in for rex: with `--autostart=false do -e <script> --json`
    /// it runs `body`; any other arguments exit 64.
    fn fake_rex(dir: &Path, body: &str) -> PathBuf {
        let script = format!(
            "#!/bin/sh
if [ \"$1 $2 $3 $5 $#\" = '--autostart=false do -e --json 5' ] && [ -n \"$4\" ]; then
{body}
else
echo \"unexpected args: $*\" >&2; exit 64
fi
"
        );
        let path = dir.join("rex");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const ANSWER: &str = r#"[{"process":{"foreground":{"name":"claude","cwd":"/work/proj","pid":42}},"program_status":{"records":[{"effective_app":"claude-code","state":"working","owner":{"pid":42}}]},"title":{"title":"T"}}]"#;

    const LONG: Duration = Duration::from_secs(5);

    #[test]
    fn the_scripts_answer_becomes_agents() {
        let d = Dir::new("ok");
        let rex = fake_rex(&d.0, &format!("cat <<'EOF'\n{ANSWER}\nEOF"));
        let agents = RexSource::new().fetch_from(&rex, LONG, &rex::Seen::default());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].workspace, "proj");
        assert_eq!(agents[0].identities, [agent_core::Identity::Pid(42)]);
    }

    #[test]
    fn no_running_server_adds_nothing() {
        let d = Dir::new("noserver");
        let rex = fake_rex(
            &d.0,
            "echo 'Error: no Rex server is running at x.sock, and autostart is disabled' >&2; exit 4",
        );
        assert!(RexSource::new()
            .fetch_from(&rex, LONG, &rex::Seen::default())
            .is_empty());
    }

    #[test]
    fn only_rexs_no_server_exit_counts_as_no_server() {
        let d = Dir::new("exits");
        let down = fake_rex(&d.0, "echo 'Error: no Rex server is running' >&2; exit 4");
        assert!(
            ask(&down, LONG, &rex::Seen::default())
                .unwrap_err()
                .no_server
        );
        let other = fake_rex(&d.0, "echo 'Error: boom' >&2; exit 1");
        assert!(
            !ask(&other, LONG, &rex::Seen::default())
                .unwrap_err()
                .no_server
        );
    }

    #[test]
    fn an_answer_that_is_not_json_adds_nothing() {
        let d = Dir::new("notjson");
        let rex = fake_rex(&d.0, "echo 'rex 9.9 usage'");
        assert!(RexSource::new()
            .fetch_from(&rex, LONG, &rex::Seen::default())
            .is_empty());
    }

    #[test]
    fn a_rex_that_never_answers_is_given_up_on() {
        let d = Dir::new("hang");
        let rex = fake_rex(&d.0, "sleep 5");
        let start = Instant::now();
        let agents =
            RexSource::new().fetch_from(&rex, Duration::from_millis(300), &rex::Seen::default());
        assert!(agents.is_empty());
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_missing_rex_yields_nothing() {
        let d = Dir::new("norex");
        assert!(RexSource::new()
            .fetch_from(&d.0.join("no-such-rex"), LONG, &rex::Seen::default())
            .is_empty());
    }
}
