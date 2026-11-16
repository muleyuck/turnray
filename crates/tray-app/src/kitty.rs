//! The kitty data source: runs `kitten @ ls` against each of kitty's remote control
//! sockets and reads the OSC 7501 records of the windows.

use std::collections::HashMap;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use agent_core::{kitty, Agent, DataSource, SourceError, Status};

use crate::process::output_with_timeout;

/// A socket whose kitty hangs would otherwise stall every poll behind it.
const KITTEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the app bundle, Homebrew and a per-user install put `kitten`. `$HOME` is added
/// by `candidates`. No login-shell lookup: it would run every second.
const KNOWN_PATHS: &[&str] = &[
    "/Applications/kitty.app/Contents/MacOS/kitten",
    "/opt/homebrew/bin/kitten",
    "/usr/local/bin/kitten",
];

fn candidates() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(KNOWN_PATHS[0])];
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join("Applications/kitty.app/Contents/MacOS/kitten"));
    }
    paths.extend(KNOWN_PATHS[1..].iter().map(PathBuf::from));
    paths
}

/// Where kitty resolves a relative `listen_on`. A `.app` has no `$TMPDIR` from a shell, so
/// it is asked of the system once.
fn temp_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os("TMPDIR").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let out = Command::new("getconf")
            .arg("DARWIN_USER_TEMP_DIR")
            .output()
            .ok()?;
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !dir.is_empty()).then(|| PathBuf::from(dir))
    })
    .as_ref()
}

/// The `kitty*` UNIX sockets directly in `dir`, by name.
fn sockets_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sockets: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("kitty"))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_socket()))
        .map(|e| e.path())
        .collect();
    sockets.sort();
    sockets
}

/// Asks one socket. The error is the reason, for the log.
fn ask(kitten: &Path, socket: &Path, timeout: Duration) -> Result<Vec<Agent>, String> {
    let mut cmd = Command::new(kitten);
    cmd.arg("@")
        .arg("--to")
        .arg(format!("unix:{}", socket.display()))
        .arg("ls");
    let out = output_with_timeout(&mut cmd, timeout).map_err(|e| match e.kind() {
        std::io::ErrorKind::TimedOut => {
            format!("kitten did not answer within {}ms", timeout.as_millis())
        }
        std::io::ErrorKind::FileTooLarge => "kitten answered with too much output".to_string(),
        _ => format!("kitten failed to start: {e}"),
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason = stderr.lines().find(|l| !l.trim().is_empty());
        return Err(match (reason, out.status.code()) {
            (Some(line), _) => format!("kitten not reachable: {}", line.trim()),
            (None, Some(code)) => format!("kitten not reachable (exit {code})"),
            (None, None) => "kitten not reachable".to_string(),
        });
    }
    kitty::parse_ls(&String::from_utf8_lossy(&out.stdout))
}

#[derive(Default)]
pub struct KittySource {
    /// The last failure logged per socket, so a persistent one isn't logged every poll.
    last_errors: Mutex<HashMap<PathBuf, String>>,
}

impl KittySource {
    pub fn new() -> KittySource {
        KittySource::default()
    }

    /// `fetch` with the kitten, the sockets and the limit given, so a test can drive it.
    fn fetch_from(&self, kitten: &Path, sockets: &[PathBuf], timeout: Duration) -> Vec<Agent> {
        if sockets.is_empty() || !kitten.exists() {
            return Vec::new();
        }
        let mut agents = Vec::new();
        let mut last = self.last_errors.lock().expect("kitty errors");
        for socket in sockets {
            match ask(kitten, socket, timeout) {
                Ok(found) => {
                    last.remove(socket);
                    agents.extend(found);
                }
                Err(reason) => {
                    if last.get(socket) != Some(&reason) {
                        tracing::warn!("kitty socket {}: {reason}", socket.display());
                        last.insert(socket.clone(), reason);
                    }
                }
            }
        }
        agents
    }
}

impl DataSource for KittySource {
    /// Never fails: no kitten, no socket or a socket that doesn't answer adds nothing.
    fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
        let Some(kitten) = candidates().into_iter().find(|p| p.exists()) else {
            return Ok(Vec::new());
        };
        let sockets = temp_dir().map(|d| sockets_in(d)).unwrap_or_default();
        Ok(self.fetch_from(&kitten, &sockets, KITTEN_TIMEOUT))
    }

    fn emitted_statuses(&self) -> &'static [Status] {
        kitty::EMITTED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    /// A short-named temp dir (socket paths are limited to 104 bytes), removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("tr-kt-{}-{name}", std::process::id()));
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

    /// One window running a claude-code record, its workspace taken from `cwd`.
    fn ls_json(cwd: &str) -> String {
        format!(
            r#"[{{"id":1,"tabs":[{{"id":1,"title":"t","windows":[{{"id":1,"title":"w","cwd":"{cwd}","foreground_processes":[{{"cmdline":["claude"],"cwd":"{cwd}","pid":42}}],"program_status":[{{"app":"claude-code","id":"","state":"working","title":"T"}}]}}]}}]}}]"#
        )
    }

    /// A shell script standing in for kitten that answers `@ --to unix:<dir>/<name> ls`
    /// with the body given for that socket name; any other arguments exit 64.
    fn fake_kitten(dir: &Path, arms: &[(&str, &str)]) -> PathBuf {
        let mut script = String::from("#!/bin/sh\ncase \"$*\" in\n");
        for (name, body) in arms {
            script += &format!("\"@ --to unix:{}/{name} ls\")\n{body}\n;;\n", dir.display());
        }
        script += "*) echo \"unexpected args: $*\" >&2; exit 64 ;;\nesac\n";
        let path = dir.join("kitten");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn prints(json: &str) -> String {
        format!("cat <<'EOF'\n{json}\nEOF")
    }

    fn sockets(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| dir.join(n)).collect()
    }

    fn workspaces(agents: &[Agent]) -> Vec<&str> {
        agents.iter().map(|a| a.workspace.as_str()).collect()
    }

    const LONG: Duration = Duration::from_secs(5);

    #[test]
    fn the_answers_of_two_sockets_follow_the_socket_order() {
        let d = Dir::new("two");
        let kitten = fake_kitten(
            &d.0,
            &[
                ("kitty-a", &prints(&ls_json("/work/first"))),
                ("kitty-b", &prints(&ls_json("/work/second"))),
            ],
        );
        let agents =
            KittySource::new().fetch_from(&kitten, &sockets(&d.0, &["kitty-a", "kitty-b"]), LONG);
        assert_eq!(workspaces(&agents), ["first", "second"]);
        assert_eq!(agents[0].identities, [agent_core::Identity::Pid(42)]);
    }

    #[test]
    fn a_socket_that_fails_to_connect_adds_nothing() {
        let d = Dir::new("fail");
        let kitten = fake_kitten(
            &d.0,
            &[
                ("kitty-a", "echo 'Failed to connect' >&2; exit 1"),
                ("kitty-b", &prints(&ls_json("/work/second"))),
            ],
        );
        let agents =
            KittySource::new().fetch_from(&kitten, &sockets(&d.0, &["kitty-a", "kitty-b"]), LONG);
        assert_eq!(workspaces(&agents), ["second"]);
    }

    #[test]
    fn a_socket_answering_with_something_else_than_json_adds_nothing() {
        let d = Dir::new("notjson");
        let kitten = fake_kitten(
            &d.0,
            &[
                ("kitty-a", "echo 'kitty 9.9 usage'"),
                ("kitty-b", &prints(&ls_json("/work/second"))),
            ],
        );
        let agents =
            KittySource::new().fetch_from(&kitten, &sockets(&d.0, &["kitty-a", "kitty-b"]), LONG);
        assert_eq!(workspaces(&agents), ["second"]);
    }

    #[test]
    fn a_socket_that_never_answers_does_not_hold_back_the_next() {
        let d = Dir::new("hang");
        let kitten = fake_kitten(
            &d.0,
            &[
                ("kitty-a", "sleep 5"),
                ("kitty-b", &prints(&ls_json("/work/second"))),
            ],
        );
        let start = Instant::now();
        let agents = KittySource::new().fetch_from(
            &kitten,
            &sockets(&d.0, &["kitty-a", "kitty-b"]),
            Duration::from_millis(300),
        );
        assert_eq!(workspaces(&agents), ["second"]);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn only_kitty_named_unix_sockets_are_picked_up() {
        let d = Dir::new("pick");
        std::fs::write(d.0.join("kitty-1"), "").unwrap();
        let _other = UnixListener::bind(d.0.join("other")).unwrap();
        let _kitty = UnixListener::bind(d.0.join("kitty-2")).unwrap();
        assert_eq!(sockets_in(&d.0), vec![d.0.join("kitty-2")]);
    }

    #[test]
    fn sockets_are_listed_in_name_order() {
        let d = Dir::new("order");
        let _b = UnixListener::bind(d.0.join("kitty-b")).unwrap();
        let _a = UnixListener::bind(d.0.join("kitty-a")).unwrap();
        assert_eq!(
            sockets_in(&d.0),
            vec![d.0.join("kitty-a"), d.0.join("kitty-b")]
        );
    }

    #[test]
    fn a_missing_kitten_yields_nothing() {
        let d = Dir::new("nokitten");
        let agents = KittySource::new().fetch_from(
            &d.0.join("no-such-kitten"),
            &sockets(&d.0, &["kitty-a"]),
            LONG,
        );
        assert!(agents.is_empty());
    }
}
