use lazy_static::lazy_static;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

// Global recording flag so UI components can display a badge without
// plumbing additional state through layers.
lazy_static! {
    pub static ref VOICE_ACTIVE: AtomicBool = AtomicBool::new(false);
}

fn debug_log(msg: &str) {
    if std::env::var("CODEX_VOICE_DEBUG")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false) 
    {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("voice_debug.log")
            .and_then(|mut f| writeln!(f, "[{}] {}", timestamp, msg));
    }
}

/// Controls launching/stopping the external whisper recorder and delivering
/// the final transcript back into the UI as a paste event.
pub(crate) struct WhisperController {
    whisper_exe: PathBuf,
    whisper_cwd: PathBuf,
    child_pid: Option<u32>,
    child_stdin: Option<std::process::ChildStdin>,
    fifo_path: Option<PathBuf>,
    reader_thread: Option<JoinHandle<()>>,
    active: Arc<AtomicBool>,
}

impl WhisperController {
    pub(crate) fn new(whisper_exe: PathBuf, whisper_cwd: PathBuf) -> Self {
        Self {
            whisper_exe,
            whisper_cwd,
            child_pid: None,
            child_stdin: None,
            fifo_path: None,
            reader_thread: None,
            active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Start whisper in headless mode. If already active, this is a no-op.
    pub(crate) fn start(&mut self, app_event_tx: AppEventSender) {
        if self.is_active() {
            return;
        }

        // Validate executable exists and is a file we can spawn.
        if !self.whisper_exe.exists() || !self.whisper_exe.is_file() {
            let msg = format!(
                "Voice: whisper executable not found at {}. Set CODEX_VOICE_WHISPER_EXE or CODEX_VOICE_WHISPER_DIR.",
                self.whisper_exe.display()
            );
            app_event_tx.send(AppEvent::LatestLog(msg));
            return;
        }

        // Create a unique FIFO for this recording session
        let fifo_path = std::env::temp_dir().join(format!("whisper_transcript_{}", std::process::id()));
        debug_log(&format!("creating FIFO at {}", fifo_path.display()));
        
        // Create the named pipe
        match nix::unistd::mkfifo(&fifo_path, nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR) {
            Ok(_) => {
                debug_log("FIFO created successfully");
                self.fifo_path = Some(fifo_path.clone());
            }
            Err(e) => {
                debug_log(&format!("failed to create FIFO: {}", e));
                app_event_tx.send(AppEvent::LatestLog(format!("Voice: failed to create FIFO: {}", e)));
                return;
            }
        }

        // If the target is a script, invoke via an interpreter.
        let mut cmd = match self.whisper_exe.extension().and_then(|e| e.to_str()) {
            Some("sh") => {
                let mut c = Command::new("bash");
                c.arg(&self.whisper_exe);
                c
            }
            Some("py") | Some("py3") => {
                let mut c = Command::new("python3");
                c.arg(&self.whisper_exe);
                c
            }
            _ => Command::new(&self.whisper_exe),
        };
        cmd.arg("--headless")
            .current_dir(&self.whisper_cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null()) // We don't need stdout anymore
            .stderr(Stdio::piped()) // Temporarily capture stderr for debugging
            .env("WHISPER_TRANSCRIPT_FIFO", &fifo_path) // Tell whisper where to write transcript
            .process_group(0); // Create new process group

        match cmd.spawn() {
            Ok(mut child) => {
                let child_pid = child.id();
                let stdin = child.stdin.take();
                let stderr = child.stderr.take();
                
                self.child_pid = Some(child_pid);
                self.child_stdin = stdin;

                let active_flag = self.active.clone();
                let fifo_path_clone = fifo_path.clone();

                // Spawn stderr reader thread to capture debug messages
                if let Some(stderr) = stderr {
                    std::thread::spawn(move || {
                        use std::io::BufRead;
                        let reader = std::io::BufReader::new(stderr);
                        for line in reader.lines().flatten() {
                            debug_log(&format!("whisper stderr: {}", line));
                        }
                    });
                }

                // Reader thread: start reading from FIFO immediately and wait for process to complete
                let handle = std::thread::spawn(move || {
                    // Signal active
                    active_flag.store(true, Ordering::SeqCst);
                    VOICE_ACTIVE.store(true, Ordering::SeqCst);
                    // Ensure UI updates promptly when recording begins.
                    app_event_tx.send(AppEvent::RequestRedraw);
                    // Also surface a visible status line now that VOICE_ACTIVE is true.
                    app_event_tx.send(AppEvent::LatestLog(
                        "🎙 Voice recording… (F9 to stop)".to_string(),
                    ));

                    // Start reading from FIFO immediately in a separate thread to avoid blocking
                    debug_log("starting FIFO reader thread");
                    let (tx, rx) = std::sync::mpsc::channel();
                    let fifo_path_for_thread = fifo_path_clone.clone();
                    
                    std::thread::spawn(move || {
                        debug_log("FIFO reader: opening FIFO for reading");
                        let result = match std::fs::File::open(&fifo_path_for_thread) {
                            Ok(mut fifo_file) => {
                                debug_log("FIFO reader: FIFO opened, reading content");
                                let mut content = String::new();
                                match std::io::Read::read_to_string(&mut fifo_file, &mut content) {
                                    Ok(_) => {
                                        debug_log(&format!("FIFO reader: read {} bytes", content.len()));
                                        Ok(content)
                                    },
                                    Err(e) => {
                                        debug_log(&format!("FIFO reader: read error: {}", e));
                                        Err(format!("error reading from FIFO: {}", e))
                                    },
                                }
                            }
                            Err(e) => {
                                debug_log(&format!("FIFO reader: open error: {}", e));
                                Err(format!("failed to open FIFO: {}", e))
                            },
                        };
                        debug_log("FIFO reader: sending result to main thread");
                        let _ = tx.send(result);
                    });

                    // Wait for the child process to complete
                    debug_log("waiting for child process to exit");
                    let exit_status = child.wait();
                    debug_log(&format!("child process exited with {:?}", exit_status));

                    // Now wait for FIFO read result with timeout
                    debug_log("waiting for FIFO read result");
                    let mut transcript = String::new();
                    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                        Ok(Ok(content)) => {
                            transcript = content;
                            debug_log(&format!("received {} bytes from FIFO reader", transcript.len()));
                        }
                        Ok(Err(e)) => {
                            debug_log(&format!("FIFO read error: {}", e));
                        }
                        Err(_) => {
                            debug_log("FIFO read timed out after process exit");
                        }
                    }

                    // Clean up the transcript
                    let final_transcript = transcript.trim().to_string();

                    debug_log(&format!(
                        "final transcript length: {} bytes, content: '{}'",
                        final_transcript.len(),
                        final_transcript.chars().take(200).collect::<String>()
                    ));

                    // Clear the status overlay before delivering paste so the composer is active.
                    app_event_tx.send(AppEvent::LatestLog(String::new()));

                    // Deliver transcript back to the app as a paste.
                    if !final_transcript.trim().is_empty() {
                        debug_log(&format!("pasting transcript: {} bytes", final_transcript.len()));
                        app_event_tx.send(AppEvent::Paste(final_transcript));
                    } else {
                        debug_log("no transcript received from FIFO");
                        app_event_tx.send(AppEvent::LatestLog(
                            "Voice: no transcript received".to_string(),
                        ));
                    }

                    // Clear active when done.
                    active_flag.store(false, Ordering::SeqCst);
                    VOICE_ACTIVE.store(false, Ordering::SeqCst);
                    app_event_tx.send(AppEvent::RequestRedraw);
                });

                self.reader_thread = Some(handle);
                // Child is moved into the reader thread, so don't store it here
            }
            Err(e) => {
                // Surface an error via LatestLog so the user sees it inline.
                let msg = format!(
                    "Voice: failed to start whisper at {}: {e}. If this is a script, ensure it has a shebang or use .sh/.py extension.",
                    self.whisper_exe.display()
                );
                app_event_tx.send(AppEvent::LatestLog(msg));
            }
        }
    }

    /// Stop whisper recording by sending SIGINT/SIGTERM to the process group. If not active, no-op.
    pub(crate) fn stop(&mut self) {
        if !self.is_active() {
            return;
        }

        debug_log("stop() called, closing stdin then sending signals");

        // First, close stdin to signal the whisper process to stop recording
        if let Some(stdin) = self.child_stdin.take() {
            debug_log("closing stdin to signal end of recording");
            drop(stdin); // This signals EOF to whisper
        }

        if let Some(pid) = self.child_pid.take() {
            // Send SIGINT to the entire process group (negative PID)
            let pgid = nix::unistd::Pid::from_raw(-(pid as i32));
            
            match nix::sys::signal::kill(pgid, nix::sys::signal::Signal::SIGINT) {
                Ok(_) => {
                    debug_log("sent SIGINT to process group");
                }
                Err(e) => {
                    debug_log(&format!("failed to send SIGINT: {}", e));
                }
            }

            // Give the process more time to handle SIGINT gracefully and complete transcription
            debug_log("giving process time to handle SIGINT gracefully and transcribe audio");
            std::thread::sleep(std::time::Duration::from_millis(3000));

            // Check if process is still running before sending SIGTERM (signal 0 = check if process exists)
            match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None) {
                Ok(_) => {
                    // Process is still running, send SIGTERM
                    debug_log("process still running, sending SIGTERM as fallback");
                    match nix::sys::signal::kill(pgid, nix::sys::signal::Signal::SIGTERM) {
                        Ok(_) => {
                            debug_log("sent SIGTERM to process group as fallback");
                        }
                        Err(_) => {
                            debug_log("SIGTERM failed");
                        }
                    }
                }
                Err(_) => {
                    // Process already exited, no need for SIGTERM
                    debug_log("process already exited gracefully, no SIGTERM needed");
                }
            }
        } else {
            debug_log("no child PID available");
        }

        // Wait for the reader thread to finish processing the final output
        if let Some(handle) = self.reader_thread.take() {
            debug_log("waiting for reader thread to complete");
            let _ = handle.join();
        }

        // Clean up FIFO
        if let Some(fifo_path) = self.fifo_path.take() {
            debug_log("cleaning up FIFO");
            let _ = std::fs::remove_file(&fifo_path);
        }

        self.child_pid = None;
        self.child_stdin = None;
        self.active.store(false, Ordering::SeqCst);
        VOICE_ACTIVE.store(false, Ordering::SeqCst);
    }
}

impl Drop for WhisperController {
    fn drop(&mut self) {
        // Best-effort stop on drop to avoid orphaned processes.
        if self.is_active() {
            self.stop();
        }
    }
}
