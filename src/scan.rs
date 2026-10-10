//! Directory listing on a pool of threads.
//!
//! Each job reads one directory and sends each folder as it is found. The
//! window schedules the jobs, so a click can move on before the walk finishes.
//! Symlinks to folders are reported and not walked unless the user opens them:
//! following them from here would loop.

use std::collections::VecDeque;
use std::fs::DirEntry;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub enum ScanEvent {
    Child {
        parent: PathBuf,
        name: String,
        path: PathBuf,
        symlink: bool,
        gen: u64,
    },
    Finished {
        path: PathBuf,
        gen: u64,
    },
    Failed {
        path: PathBuf,
        gen: u64,
        message: String,
    },
}

struct Job {
    path: PathBuf,
    gen: u64,
}

struct State {
    jobs: VecDeque<Job>,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

pub struct Found {
    pub name: String,
    pub path: PathBuf,
    pub symlink: bool,
}

pub struct Scanner {
    shared: Arc<Shared>,
    events: mpsc::Receiver<ScanEvent>,
    threads: Vec<JoinHandle<()>>,
}

impl Scanner {
    pub fn spawn() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                jobs: VecDeque::new(),
                stop: false,
            }),
            cv: Condvar::new(),
        });
        let (tx, events) = mpsc::channel();
        let count = worker_count();
        let mut threads = Vec::with_capacity(count);
        for i in 0..count {
            let shared = Arc::clone(&shared);
            let tx = tx.clone();
            let handle = thread::Builder::new()
                .name(format!("nav-scan-{i}"))
                .spawn(move || worker(shared, tx))
                .expect("spawn scanner thread");
            threads.push(handle);
        }
        Self {
            shared,
            events,
            threads,
        }
    }

    pub fn enqueue(&self, path: PathBuf, gen: u64) {
        let mut guard = self.shared.state.lock().expect("scanner lock");
        if guard.stop {
            return;
        }
        guard.jobs.push_back(Job { path, gen });
        self.shared.cv.notify_one();
    }

    pub fn poll(&self) -> Vec<ScanEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        {
            let mut guard = self.shared.state.lock().expect("scanner lock");
            guard.stop = true;
            guard.jobs.clear();
        }
        self.shared.cv.notify_all();
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

fn worker_count() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

fn worker(shared: Arc<Shared>, tx: mpsc::Sender<ScanEvent>) {
    loop {
        let job = {
            let mut guard = shared.state.lock().expect("scanner lock");
            loop {
                if guard.stop {
                    return;
                }
                if let Some(job) = guard.jobs.pop_front() {
                    break job;
                }
                guard = shared.cv.wait(guard).expect("scanner wait");
            }
        };
        if scan_one(&job, &tx).is_err() {
            return;
        }
    }
}

fn scan_one(job: &Job, tx: &mpsc::Sender<ScanEvent>) -> Result<(), ()> {
    let entries = match std::fs::read_dir(&job.path) {
        Ok(entries) => entries,
        Err(err) => {
            tx.send(ScanEvent::Failed {
                path: job.path.clone(),
                gen: job.gen,
                message: err.to_string(),
            })
            .map_err(|_| ())?;
            return Ok(());
        }
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Some(found) = classify(&entry) else { continue };
        tx.send(ScanEvent::Child {
            parent: job.path.clone(),
            name: found.name,
            path: found.path,
            symlink: found.symlink,
            gen: job.gen,
        })
        .map_err(|_| ())?;
    }
    tx.send(ScanEvent::Finished {
        path: job.path.clone(),
        gen: job.gen,
    })
    .map_err(|_| ())
}

fn classify(entry: &DirEntry) -> Option<Found> {
    let file_type = entry.file_type().ok()?;
    let name = entry.file_name().to_string_lossy().into_owned();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let path = entry.path();
    if file_type.is_dir() {
        return Some(Found {
            name,
            path,
            symlink: false,
        });
    }
    // A symlink's own type is not a directory. Follow it once so a linked
    // folder can be opened, and leave the walk to the click.
    if file_type.is_symlink() && std::fs::metadata(&path).is_ok_and(|meta| meta.is_dir()) {
        return Some(Found {
            name,
            path,
            symlink: true,
        });
    }
    None
}

#[cfg(test)]
pub fn list_children(path: &Path) -> Result<Vec<Found>, String> {
    let entries = std::fs::read_dir(path).map_err(|err| err.to_string())?;
    let mut found = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        if let Some(item) = classify(&entry) {
            found.push(item);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("nav-{label}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_tree(root: &Path) {
        fs::create_dir_all(root.join("alpha/one")).unwrap();
        fs::create_dir_all(root.join("alpha/two")).unwrap();
        fs::write(root.join("alpha/file.txt"), b"notes").unwrap();
        fs::create_dir_all(root.join("beta")).unwrap();
        fs::write(root.join("notes.txt"), b"skip me").unwrap();
        std::os::unix::fs::symlink("alpha", root.join("link_alpha")).unwrap();
        std::os::unix::fs::symlink("notes.txt", root.join("link_file")).unwrap();
    }

    #[test]
    fn list_children_keeps_directories_and_drops_files() {
        let tmp = Tmp::new("list");
        sample_tree(&tmp.0);
        let mut names: Vec<_> = list_children(&tmp.0)
            .unwrap()
            .into_iter()
            .map(|item| (item.name, item.symlink))
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            names,
            vec![
                ("alpha".to_string(), false),
                ("beta".to_string(), false),
                ("link_alpha".to_string(), true),
            ]
        );
        let nested: Vec<_> = list_children(&tmp.0.join("alpha"))
            .unwrap()
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert!(nested.contains(&"one".to_string()));
        assert!(nested.contains(&"two".to_string()));
        assert!(!nested.iter().any(|name| name == "file.txt"));
    }

    #[test]
    fn pool_reports_children_then_finished_and_a_missing_path_fails() {
        let tmp = Tmp::new("pool");
        sample_tree(&tmp.0);
        let scanner = Scanner::spawn();
        scanner.enqueue(tmp.0.clone(), 7);
        scanner.enqueue(PathBuf::from("/no/such/nav/directory"), 8);

        let start = Instant::now();
        let mut children = Vec::new();
        let mut finished = false;
        let mut failed = false;
        while start.elapsed() < Duration::from_secs(5) && !(finished && failed) {
            for event in scanner.poll() {
                match event {
                    ScanEvent::Child { name, gen, .. } if gen == 7 => children.push(name),
                    ScanEvent::Finished { gen, .. } if gen == 7 => finished = true,
                    ScanEvent::Failed { gen, .. } if gen == 8 => failed = true,
                    _ => {}
                }
            }
            if !(finished && failed) {
                thread::sleep(Duration::from_millis(10));
            }
        }
        children.sort();
        assert_eq!(children, vec!["alpha".to_string(), "beta".to_string(), "link_alpha".to_string()]);
        assert!(finished);
        assert!(failed);
    }
}
