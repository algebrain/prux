use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use prux::spawn::{spawn_process, SpawnConfig, SpawnMode};
use prux::{ProcessSession, ProcessSessionConfig};

fn spawn_shell() -> ProcessSession {
    ProcessSession::spawn(ProcessSessionConfig {
        program: PathBuf::from("/bin/sh"),
        args: Vec::new(),
        initial_cwd: std::env::current_dir().unwrap(),
        debug_log_path: None,
        env: Default::default(),
    })
    .unwrap()
}

fn spawn_interactive_bash() -> ProcessSession {
    let mut env = BTreeMap::new();
    env.insert("PS1".to_string(), "__PRUX_PROMPT__ ".to_string());
    ProcessSession::spawn(ProcessSessionConfig {
        program: PathBuf::from("bash"),
        args: vec![
            "--noprofile".to_string(),
            "--norc".to_string(),
            "-i".to_string(),
        ],
        initial_cwd: std::env::current_dir().unwrap(),
        debug_log_path: None,
        env,
    })
    .unwrap()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{}-{}-{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn wait_for_spawn_output(master: &prux::pty::PtyMaster, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut output = String::new();
    while Instant::now() < deadline {
        let chunk = master.try_read().unwrap();
        if !chunk.is_empty() {
            output.push_str(&String::from_utf8_lossy(&chunk));
            if output.contains(needle) {
                return output;
            }
        } else {
            thread::sleep(Duration::from_millis(20));
        }
    }
    panic!("timed out waiting for output containing {needle:?}; got {output:?}");
}

fn wait_for_output(session: &mut ProcessSession, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while Instant::now() < deadline {
        let chunk = session.try_read().unwrap();
        if !chunk.is_empty() {
            output.push_str(&String::from_utf8_lossy(&chunk));
            if output.contains(needle) {
                return output;
            }
        } else {
            thread::sleep(Duration::from_millis(20));
        }
    }

    panic!("timed out waiting for output containing {needle:?}; got {output:?}");
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn shell_round_trip_io_works() {
    let mut session = spawn_shell();
    session.write_all(b"printf 'READY\\n'\n").unwrap();
    let output = wait_for_output(&mut session, "READY");
    assert!(output.contains("READY"));
}

#[test]
fn current_dir_matches_initial_cwd() {
    let mut session = spawn_shell();
    let cwd = std::env::current_dir().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let reported = loop {
        match session.current_dir() {
            Ok(path) => break path,
            Err(err) if Instant::now() < deadline => {
                assert!(err.to_string().contains("current_dir"));
                thread::sleep(Duration::from_millis(20));
            }
            Err(err) => panic!("current_dir did not stabilize: {err}"),
        }
    };
    assert_eq!(reported, cwd);
    session.terminate().unwrap();
}

#[test]
fn current_dir_tracks_foreground_child_workload_like_tmux_osdep_linux() {
    let temp = unique_temp_dir("prux-cwd");
    let shell_dir = temp.join("shell");
    let child_dir = temp.join("child");
    std::fs::create_dir(&shell_dir).unwrap();
    std::fs::create_dir(&child_dir).unwrap();

    let mut session = ProcessSession::spawn(ProcessSessionConfig {
        program: PathBuf::from("/bin/sh"),
        args: Vec::new(),
        initial_cwd: shell_dir.clone(),
        debug_log_path: None,
        env: Default::default(),
    })
    .unwrap();

    let initial_ok = wait_until(Duration::from_secs(2), || {
        session
            .current_dir()
            .map(|cwd| cwd == shell_dir)
            .unwrap_or(false)
    });
    assert!(initial_ok, "shell cwd did not stabilize to initial dir");

    session
        .write_all(format!("sh -c 'cd \"{}\"; sleep 1' \n", child_dir.display()).as_bytes())
        .unwrap();
    thread::sleep(Duration::from_millis(150));

    let child_seen = wait_until(Duration::from_secs(1), || {
        session
            .current_dir()
            .map(|cwd| cwd == child_dir)
            .unwrap_or(false)
    });
    assert!(
        child_seen,
        "foreground child cwd was not observed via PTY foreground pgrp semantics"
    );

    let shell_back = wait_until(Duration::from_secs(2), || {
        session
            .current_dir()
            .map(|cwd| cwd == shell_dir)
            .unwrap_or(false)
    });
    assert!(
        shell_back,
        "shell cwd did not return after foreground child exit"
    );
    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn resize_changes_child_tty_size() {
    let mut session = spawn_shell();
    session.resize(100, 40).unwrap();
    session.write_all(b"stty size\n").unwrap();
    let output = wait_for_output(&mut session, "40 100");
    assert!(output.contains("40 100"));
}

#[test]
fn interrupt_keeps_shell_usable() {
    let mut session = spawn_shell();
    session.write_all(b"sleep 10\n").unwrap();
    thread::sleep(Duration::from_millis(150));
    session.send_interrupt().unwrap();
    thread::sleep(Duration::from_millis(150));
    session.write_all(b"printf 'AFTER\\n'\n").unwrap();
    let output = wait_for_output(&mut session, "AFTER");
    assert!(output.contains("AFTER"));
}

#[test]
fn terminate_marks_session_dead() {
    let mut session = spawn_shell();
    assert!(session.is_alive().unwrap());
    session.terminate().unwrap();
    assert!(!session.is_alive().unwrap());
}

#[test]
fn shell_can_be_spawned_via_path_lookup() {
    let mut session = ProcessSession::spawn(ProcessSessionConfig {
        program: PathBuf::from("sh"),
        args: Vec::new(),
        initial_cwd: std::env::current_dir().unwrap(),
        debug_log_path: None,
        env: Default::default(),
    })
    .unwrap();

    session.write_all(b"printf 'PATHOK\\n'\n").unwrap();
    let output = wait_for_output(&mut session, "PATHOK");
    assert!(output.contains("PATHOK"));
}

#[test]
fn exited_process_is_reaped_and_marked_dead() {
    let mut session = ProcessSession::spawn(ProcessSessionConfig {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), "exit 7".to_string()],
        initial_cwd: std::env::current_dir().unwrap(),
        debug_log_path: None,
        env: Default::default(),
    })
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !session.is_alive().unwrap() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    panic!("child process was not reaped in time");
}

#[test]
fn reaper_handles_child_exit_before_any_pty_read() {
    let mut session = ProcessSession::spawn(ProcessSessionConfig {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), "printf 'FAST\\n'; exit 0".to_string()],
        initial_cwd: std::env::current_dir().unwrap(),
        debug_log_path: None,
        env: Default::default(),
    })
    .unwrap();

    thread::sleep(Duration::from_millis(100));
    assert!(
        !session.is_alive().unwrap(),
        "child should already be reaped"
    );
    let output = String::from_utf8_lossy(&session.try_read().unwrap()).into_owned();
    assert!(
        output.contains("FAST"),
        "expected buffered PTY output after fast exit"
    );
}

#[test]
fn interrupt_preserves_interactive_editing_after_late_tty_corruption() {
    let mut session = spawn_interactive_bash();
    let _ = wait_for_output(&mut session, "__PRUX_PROMPT__");

    session
        .write_all(
            b"sh -c 'trap \"(sleep 0.25; stty raw -echo </dev/tty >/dev/tty) & exit 130\" INT; while :; do sleep 1; done'\n",
        )
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    session.send_interrupt().unwrap();

    let prompt_back = wait_until(Duration::from_secs(3), || {
        let chunk = session.try_read().unwrap_or_default();
        !chunk.is_empty()
    });
    assert!(
        prompt_back,
        "interactive shell did not resume after interrupt"
    );

    thread::sleep(Duration::from_millis(450));
    let _ = session.try_read().unwrap();

    session.write_all(b"echo abc\x7fd\n").unwrap();
    let output = wait_for_output(&mut session, "__PRUX_PROMPT__");
    assert!(
        output.contains("abd"),
        "expected backspace editing to survive late tty corruption; got {output:?}"
    );
    assert!(
        !output.contains("^H"),
        "shell echoed raw backspace after interrupt; got {output:?}"
    );

    let _ = session.try_read().unwrap();
    session.write_all(b"echo ac\x1b[D\x1b[DX\n").unwrap();
    let output = wait_for_output(&mut session, "__PRUX_PROMPT__");
    assert!(
        output.contains("Xac"),
        "expected left-arrow editing to survive late tty corruption; got {output:?}"
    );
    assert!(
        !output.contains("^[[D"),
        "shell echoed raw left-arrow escape after interrupt; got {output:?}"
    );
}

#[test]
fn stopped_child_is_resumed_like_tmux_server_child_stopped() {
    let mut session = ProcessSession::spawn(ProcessSessionConfig {
        program: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".to_string(),
            "kill -STOP $$; printf 'RESUMED\\n'".to_string(),
        ],
        initial_cwd: std::env::current_dir().unwrap(),
        debug_log_path: None,
        env: Default::default(),
    })
    .unwrap();

    let output = wait_for_output(&mut session, "RESUMED");
    assert!(
        output.contains("RESUMED"),
        "stopped child was not resumed by reaper logic"
    );
}

#[test]
fn interrupt_does_not_leave_same_tty_orphan_group_that_breaks_editing() {
    let mut session = spawn_interactive_bash();
    let _ = wait_for_output(&mut session, "__PRUX_PROMPT__");

    session
        .write_all(
            b"sh -c 'trap \"exit 130\" INT; (sh -c \"trap \\\"\\\" HUP INT TERM; while :; do sleep 1; done\") & while :; do sleep 1; done'\n",
        )
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    session.send_interrupt().unwrap();

    let prompt_back = wait_until(Duration::from_secs(3), || {
        let chunk = session.try_read().unwrap_or_default();
        !chunk.is_empty()
    });
    assert!(prompt_back, "shell did not resume after interrupt");

    let _ = session.try_read().unwrap();
    session.write_all(b"echo ac\x1b[D\x1b[DX\n").unwrap();
    let output = wait_for_output(&mut session, "__PRUX_PROMPT__");
    assert!(
        output.contains("Xac"),
        "editing degraded after interrupted same-tty group; got {output:?}"
    );
    assert!(
        !output.contains("^[[D"),
        "shell echoed raw left-arrow after interrupted same-tty group; got {output:?}"
    );
}

#[test]
fn spawn_mode_shell_command_matches_tmux_single_argument_path() {
    let spawned = spawn_process(SpawnConfig {
        program: PathBuf::from("ignored-for-shell-command"),
        args: vec!["printf 'SHELLCMD\\n'".to_string()],
        cwd: std::env::current_dir().unwrap(),
        env: Default::default(),
        termios_template: prux::os::linux::capture_stdin_termios().ok(),
        mode: SpawnMode::ShellCommand,
    })
    .unwrap();

    let output = wait_for_spawn_output(&spawned.master, "SHELLCMD", Duration::from_secs(3));
    assert!(output.contains("SHELLCMD"));
}

#[test]
fn spawn_mode_login_shell_matches_tmux_zero_argument_path() {
    let spawned = spawn_process(SpawnConfig {
        program: PathBuf::from("ignored-for-login-shell"),
        args: Vec::new(),
        cwd: std::env::current_dir().unwrap(),
        env: Default::default(),
        termios_template: prux::os::linux::capture_stdin_termios().ok(),
        mode: SpawnMode::LoginShell,
    })
    .unwrap();

    spawned.master.write_all(b"printf 'LOGINOK\\n'\n").unwrap();
    let output = wait_for_spawn_output(&spawned.master, "LOGINOK", Duration::from_secs(3));
    assert!(output.contains("LOGINOK"));
}
