//! Runs a child process with a deadline and a cap on its output.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// How long to wait for the drain threads once the child has exited.
const EXIT_GRACE: Duration = Duration::from_millis(200);

/// Far above any real agent list. A child printing more is read to its end but not kept,
/// so it neither blocks on a full pipe nor grows memory without bound.
const MAX_OUTPUT: usize = 4 * 1024 * 1024;

/// `Command::output` with a deadline: past it the child's whole process group is killed
/// and `TimedOut` returned. Its pipes are drained on their own threads so a chatty child
/// can't block on a full pipe; more than `MAX_OUTPUT` on either returns `FileTooLarge`.
pub fn output_with_timeout(cmd: &mut Command, timeout: Duration) -> std::io::Result<Output> {
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
    match (stdout.collect(until), stderr.collect(until)) {
        (Some(stdout), Some(stderr)) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        _ => Err(std::io::ErrorKind::FileTooLarge.into()),
    }
}

/// Reads a pipe to its end on a thread, keeping what it has read so far reachable.
struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    overflowed: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
}

impl Drain {
    fn spawn(pipe: Option<impl Read + Send + 'static>) -> Drain {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let overflowed = Arc::new(AtomicBool::new(false));
        let (tx, done) = mpsc::channel();
        let (shared, over) = (Arc::clone(&buf), Arc::clone(&overflowed));
        std::thread::spawn(move || {
            if let Some(pipe) = pipe {
                read_capped(pipe, &shared, &over);
            }
            let _ = tx.send(());
        });
        Drain {
            buf,
            overflowed,
            done,
        }
    }

    /// Waits until `until` at most for the end of the pipe, then returns what has arrived,
    /// or `None` if it was more than `MAX_OUTPUT`.
    fn collect(self, until: Instant) -> Option<Vec<u8>> {
        let _ = self
            .done
            .recv_timeout(until.saturating_duration_since(Instant::now()));
        if self.overflowed.load(Ordering::Relaxed) {
            return None;
        }
        Some(std::mem::take(&mut *self.buf.lock().expect("drain buffer")))
    }
}

/// Reads `pipe` to its end into `buf`. Past `MAX_OUTPUT` it keeps reading but drops
/// everything and sets `overflowed`.
fn read_capped(mut pipe: impl Read, buf: &Mutex<Vec<u8>>, overflowed: &AtomicBool) {
    let mut chunk = [0; 4096];
    while let Ok(n @ 1..) = pipe.read(&mut chunk) {
        let mut buf = buf.lock().expect("drain buffer");
        if buf.len() + n > MAX_OUTPUT {
            overflowed.store(true, Ordering::Relaxed);
            buf.clear();
        } else if !overflowed.load(Ordering::Relaxed) {
            buf.extend_from_slice(&chunk[..n]);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
