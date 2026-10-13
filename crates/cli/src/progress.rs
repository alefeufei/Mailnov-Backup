//! Consumes engine `Event`s on a separate thread: an `indicatif` bar on a TTY, plain lines otherwise.
use indicatif::{ProgressBar, ProgressStyle};
use mailbackup_core::Event;
use std::io::IsTerminal;
use std::sync::mpsc::{channel, Sender};

/// A message (`uid` is `Some`) or a whole folder (`uid` is `None`) the engine could not process.
#[derive(Debug, Clone)]
pub struct Failure {
    pub folder: String,
    pub uid: Option<u32>,
    pub reason: String,
}

/// Runs `work` on the calling thread with an event sender while a second thread renders progress.
/// The consumer is joined before this returns; the failures it saw are returned with the result.
pub fn with_progress<T>(label: &str, work: impl FnOnce(Sender<Event>) -> T) -> (T, Vec<Failure>) {
    let (tx, rx) = channel::<Event>();
    let tty = std::io::stdout().is_terminal();
    let label = label.to_string();
    let consumer = std::thread::spawn(move || {
        let mut failures = Vec::new();
        let bar = if tty {
            let b = ProgressBar::new_spinner();
            b.set_style(
                ProgressStyle::with_template("{spinner} {msg} [{pos}/{len}]")
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            b.set_message(label.clone());
            Some(b)
        } else {
            None
        };
        for ev in rx {
            match ev {
                Event::FolderStarted { folder, total } => match &bar {
                    Some(b) => {
                        b.set_message(format!("{label}: {folder}"));
                        b.set_position(0);
                        b.set_length(total as u64);
                    }
                    None => println!("{label}: folder {folder}: {total} message(s)"),
                },
                Event::MessageArchived { .. } => {
                    if let Some(b) = &bar {
                        b.inc(1);
                    }
                }
                Event::MessageFailed {
                    folder,
                    uid,
                    reason,
                } => {
                    if let Some(b) = &bar {
                        b.inc(1);
                    }
                    failures.push(Failure {
                        folder,
                        uid: Some(uid),
                        reason,
                    });
                }
                Event::FolderFailed { folder, reason } => {
                    if bar.is_none() {
                        println!("{label}: folder {folder}: failed: {reason}");
                    }
                    failures.push(Failure {
                        folder,
                        uid: None,
                        reason,
                    });
                }
                Event::Finished(_) => {}
            }
        }
        if let Some(b) = bar {
            b.finish_and_clear();
        }
        failures
    });
    let result = work(tx);
    let failures = consumer.join().unwrap_or_default();
    (result, failures)
}
