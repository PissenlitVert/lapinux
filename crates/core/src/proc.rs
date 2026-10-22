//! Exécution de processus externes avec capture de sortie en continu.

use crate::error::{Error, Result};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::thread;

#[derive(Debug, Clone)]
pub struct StreamOutcome {
    pub status: ExitStatus,
    pub lines: Vec<String>,
}

impl StreamOutcome {
    pub fn success(&self) -> bool {
        self.status.success()
    }
}

/// Lance un processus et transmet chaque ligne de stdout/stderr au callback.
///
/// Le callback renvoie `true` pour continuer, `false` pour arrêter le processus.
/// Un `Err` de spawn est remonté tel quel.
pub fn run_streaming(cmd: &mut Command, on_line: &mut dyn FnMut(&str) -> bool) -> Result<StreamOutcome> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (tx, rx) = channel::<String>();
    let mut handles = Vec::new();
    if let Some(stream) = stdout {
        handles.push(spawn_reader(stream, tx.clone()));
    }
    if let Some(stream) = stderr {
        handles.push(spawn_reader(stream, tx.clone()));
    }
    drop(tx);

    let mut lines = Vec::new();
    let mut keep_going = true;
    while let Ok(line) = rx.recv() {
        lines.push(line.clone());
        if keep_going && !on_line(&line) {
            keep_going = false;
            let _ = child.kill();
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    let status = child.wait()?;
    Ok(StreamOutcome { status, lines })
}

fn spawn_reader(stream: impl Read + Send + 'static, tx: Sender<String>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(|l| l.ok()) {
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

/// Exécute un processus et renvoie (statut, stdout, stderr).
pub fn run_capture(cmd: &mut Command) -> Result<(ExitStatus, String, String)> {
    let out = cmd.output()?;
    Ok((
        out.status,
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

/// Lancé en arrière-plan, sortie redirigée vers un fichier journal.
pub fn spawn_detached(cmd: &mut Command, log_path: &std::path::Path) -> Result<u32> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let err = log.try_clone()?;
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err))
        .spawn()?;
    Ok(child.id())
}

pub fn missing_binary(name: &str) -> Error {
    Error::Other(format!("binaire requis introuvable : {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_stdout_and_stderr() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo un; echo deux >&2; echo trois");
        let mut seen = Vec::new();
        let outcome = run_streaming(&mut cmd, &mut |line| {
            seen.push(line.to_string());
            true
        })
        .unwrap();
        assert!(outcome.success());
        assert!(seen.contains(&"un".to_string()));
        assert!(seen.contains(&"trois".to_string()));
    }

    #[test]
    fn callback_false_kills() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo a; sleep 5; echo b");
        let mut count = 0;
        let outcome = run_streaming(&mut cmd, &mut |_| {
            count += 1;
            false
        })
        .unwrap();
        assert!(!outcome.success());
        assert_eq!(count, 1);
    }

    #[test]
    fn capture_separates_streams() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo out; echo err >&2");
        let (status, out, err) = run_capture(&mut cmd).unwrap();
        assert!(status.success());
        assert_eq!(out.trim(), "out");
        assert_eq!(err.trim(), "err");
    }
}
