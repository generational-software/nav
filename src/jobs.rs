//! Commands the plaza is running. Each one is a login bash in the folder it was started from.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread;

use std::os::unix::process::CommandExt;

const MAX_JOBS: usize = 24;
const KEPT_LINES: usize = 48;
const SHOWN_LINES: usize = 3;

pub struct Jobs {
    next_id: u64,
    items: Vec<Job>,
}

struct Job {
    id: u64,
    name: String,
    dir: PathBuf,
    child: Option<Child>,
    rx: Receiver<String>,
    lines: VecDeque<String>,
    status: Status,
}

enum Status {
    Running,
    Exited(i32),
    Stopped,
}

pub struct JobView {
    pub id: u64,
    pub name: String,
    pub dir: PathBuf,
    pub lines: Vec<String>,
    pub running: bool,
    /// The command exited on its own. Stopping it with × is not finished.
    pub finished: bool,
}

impl Jobs {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            items: Vec::new(),
        }
    }

    pub fn poll(&mut self) {
        for job in &mut self.items {
            job.drain();
            job.reap();
        }
    }

    /// Start `line` in a login bash whose working directory is `dir`.
    /// The shell is its own process group, so closing the tile can stop its children too.
    pub fn start(&mut self, name: &str, dir: &Path, line: &str) -> Option<u64> {
        if self.items.len() >= MAX_JOBS {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let job = match spawn(dir, line) {
            Ok((child, rx)) => Job {
                id,
                name: name.to_string(),
                dir: dir.to_path_buf(),
                child: Some(child),
                rx,
                lines: VecDeque::new(),
                status: Status::Running,
            },
            Err(err) => {
                let (_tx, rx) = mpsc::sync_channel(1);
                Job {
                    id,
                    name: name.to_string(),
                    dir: dir.to_path_buf(),
                    child: None,
                    rx,
                    lines: VecDeque::from([format!("could not start: {err}")]),
                    status: Status::Exited(1),
                }
            }
        };
        self.items.push(job);
        Some(id)
    }

    pub fn dir_of(&self, id: u64) -> Option<PathBuf> {
        self.items.iter().find(|job| job.id == id).map(|job| job.dir.clone())
    }

    /// Command names that are still running in `dir`. A finished or stopped task is left out.
    pub fn running_names(&self, dir: &Path) -> Vec<String> {
        self.items
            .iter()
            .filter(|job| job.dir == dir && matches!(job.status, Status::Running))
            .map(|job| job.name.clone())
            .collect()
    }

    /// Stop a running command. Once it has already stopped, remove its tile.
    pub fn close(&mut self, id: u64) {
        let Some(index) = self.items.iter().position(|job| job.id == id) else {
            return;
        };
        if self.items[index].child.is_some() {
            if let Some(child) = self.items[index].child.as_mut() {
                kill_group(child);
            }
            self.items[index].child = None;
            self.items[index].status = Status::Stopped;
            self.items[index].drain();
            return;
        }
        self.items.remove(index);
    }

    pub fn views(&self) -> Vec<JobView> {
        self.items
            .iter()
            .map(|job| JobView {
                id: job.id,
                name: job.name.clone(),
                dir: job.dir.clone(),
                lines: preview(&job.lines, &job.status),
                running: matches!(job.status, Status::Running),
                finished: matches!(job.status, Status::Exited(_)),
            })
            .collect()
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        for job in &mut self.items {
            if let Some(child) = job.child.as_mut() {
                kill_group(child);
            }
        }
    }
}

impl Job {
    fn drain(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(line) => {
                    if line.is_empty() {
                        continue;
                    }
                    if self.lines.len() == KEPT_LINES {
                        self.lines.pop_front();
                    }
                    self.lines.push_back(line);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn reap(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.status = Status::Exited(status.code().unwrap_or(-1));
                self.child = None;
            }
            Ok(None) => {}
            Err(_) => {
                self.status = Status::Exited(-1);
                self.child = None;
            }
        }
    }
}

fn preview(lines: &VecDeque<String>, status: &Status) -> Vec<String> {
    let mut shown: Vec<String> = lines.iter().rev().take(SHOWN_LINES).cloned().collect();
    shown.reverse();
    let note = match status {
        Status::Running => None,
        Status::Stopped => Some("stopped".to_string()),
        Status::Exited(0) => None,
        Status::Exited(code) => Some(format!("exited {code}")),
    };
    if let Some(note) = note {
        if shown.len() == SHOWN_LINES {
            shown.pop();
        }
        shown.push(note);
    }
    if shown.is_empty() {
        shown.push(match status {
            Status::Running => "running".to_string(),
            Status::Stopped => "stopped".to_string(),
            Status::Exited(0) => "finished".to_string(),
            Status::Exited(_) => "exited".to_string(),
        });
    }
    shown
}

fn spawn(dir: &Path, line: &str) -> std::io::Result<(Child, Receiver<String>)> {
    // Login bash reads the profile first, so the command sees the same tools as a terminal.
    // The script then comes back to this folder and puts its node_modules/.bin ahead of that PATH.
    let quoted = bash_quote(&dir.to_string_lossy());
    let script = format!("cd {quoted} || exit\nPATH=\"$PWD/node_modules/.bin:$PATH\"\n{line}");
    let mut cmd = Command::new("bash");
    cmd.arg("--login")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn()?;
    let (tx, rx) = mpsc::sync_channel(256);
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || pump(out, tx));
    }
    if let Some(err) = child.stderr.take() {
        thread::spawn(move || pump(err, tx));
    }
    Ok((child, rx))
}

fn pump(mut input: impl Read, tx: SyncSender<String>) {
    let mut buf = [0u8; 1024];
    let mut pending = String::new();
    loop {
        match input.read(&mut buf) {
            Ok(0) => {
                let _ = send_line(&tx, &pending);
                break;
            }
            Ok(n) => {
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(pos) = pending.find('\n') {
                    let line: String = pending.drain(..=pos).collect();
                    if send_line(&tx, &line).is_err() {
                        return;
                    }
                }
                if pending.len() > 8000 && send_line(&tx, &pending).is_err() {
                    return;
                } else if pending.len() > 8000 {
                    pending.clear();
                }
            }
            Err(_) => break,
        }
    }
}

fn send_line(tx: &SyncSender<String>, raw: &str) -> Result<(), mpsc::SendError<String>> {
    let line = clean_line(raw);
    if line.is_empty() {
        return Ok(());
    }
    tx.send(line)
}

/// Quote a path so a login bash `cd`s to it even when the name contains spaces.
fn bash_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn kill_group(child: &mut Child) {
    let pid = child.id() as i32;
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Drop ANSI colour codes and keep the text after the last carriage return.
fn clean_line(raw: &str) -> String {
    let mut plain = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        plain.push(ch);
    }
    let last = plain.rsplit('\r').next().unwrap_or("");
    let last = last.trim_end_matches(['\n', '\r', ' ']);
    let count = last.chars().count();
    if count <= 200 {
        return last.to_string();
    }
    let head: String = last.chars().take(199).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn scratch() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nav-jobs-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn colour_codes_and_carriage_returns_do_not_reach_the_tile() {
        assert_eq!(clean_line("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(clean_line("old\rnew\n"), "new");
        assert!(clean_line("   \n").is_empty());
    }

    #[test]
    fn a_command_prints_into_its_tile_and_then_finishes() {
        let dir = scratch();
        let mut jobs = Jobs::new();
        let id = jobs.start("ping", &dir, "echo hello-from-nav").unwrap();
        let start = Instant::now();
        let view = loop {
            jobs.poll();
            let view = jobs.views().into_iter().find(|view| view.id == id).unwrap();
            if view.lines.iter().any(|line| line.contains("hello-from-nav")) && !view.running {
                break view;
            }
            if start.elapsed() > Duration::from_secs(2) {
                panic!("echo did not finish: {:?}", jobs.views().iter().map(|view| view.lines.clone()).collect::<Vec<_>>());
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(view.name, "ping");
        assert!(view.finished);
        assert!(view.lines.iter().any(|line| line == "hello-from-nav"));
        jobs.close(id);
        assert!(jobs.views().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn closing_a_running_command_stops_the_shell_and_what_it_started() {
        let dir = scratch();
        let marker = dir.join("done");
        let mut jobs = Jobs::new();
        let id = jobs.start("slow", &dir, "sleep 1; touch done").unwrap();
        let pid = jobs.items.iter().find(|job| job.id == id).unwrap().child.as_ref().unwrap().id();
        jobs.close(id);
        let view = jobs.views().into_iter().find(|view| view.id == id).unwrap();
        assert!(!view.running);
        assert!(!view.finished);
        assert!(view.lines.iter().any(|line| line == "stopped"));
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "a stopped command should not keep running");
        jobs.close(id);
        assert!(jobs.views().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_command_runs_in_a_login_bash_in_its_folder() {
        let dir = scratch().join("my folder's place");
        fs::create_dir_all(dir.join("node_modules/.bin")).unwrap();
        fs::write(dir.join("marker"), b"here").unwrap();
        let mut jobs = Jobs::new();
        let id = jobs
            .start(
                "login",
                &dir,
                "shopt -q login_shell && login=login-shell || login=not-login\ncase \"$PATH\" in \"$PWD/node_modules/.bin:\"*) bin=local-bin-first;; *) bin=local-bin-missing;; esac\nif test -f marker; then place=in-folder; else place=wrong-folder; fi\necho \"$login $bin $place\"",
            )
            .unwrap();
        let start = Instant::now();
        let view = loop {
            jobs.poll();
            let view = jobs.views().into_iter().find(|view| view.id == id).unwrap();
            if !view.running {
                break view;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("login bash did not finish: {:?}", view.lines);
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert!(
            view.lines.iter().any(|line| line == "login-shell local-bin-first in-folder"),
            "{:?}",
            view.lines
        );
        assert!(!view.lines.iter().any(|line| line.starts_with("exited")));
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn tiles_keep_the_order_they_were_started() {
        let dir = scratch();
        let mut jobs = Jobs::new();
        jobs.start("one", &dir, "echo one").unwrap();
        jobs.start("two", &dir, "echo two").unwrap();
        let names: Vec<_> = jobs.views().into_iter().map(|view| view.name).collect();
        assert_eq!(names, ["one", "two"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
