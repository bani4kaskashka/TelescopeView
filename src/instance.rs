//! One window: opening a file while TelescopeView is already open shows it in that window
//! (when "Open files in the same window" is on there). The open window listens on a local
//! port written to a small file in the temp folder; a new launch sends its path there.

use eframe::egui;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn port_file() -> PathBuf {
    std::env::temp_dir().join("TelescopeView.port")
}

/// Asks an open TelescopeView to show `path`. True when it took it (this launch can quit).
pub fn hand_off(path: &Path) -> bool {
    let Some(port) = std::fs::read_to_string(port_file()).ok().and_then(|s| s.trim().parse::<u16>().ok()) else {
        return false;
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    // Windows only lets the app the user just started bring a window to the front; pass that on.
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(u32::MAX);
    }
    if writeln!(stream, "{}", path.display()).is_err() {
        return false;
    }
    let mut reply = [0u8; 1];
    stream.read_exact(&mut reply).is_ok() && reply[0] == b'y'
}

/// Takes files from later launches while `take` is on: each goes into `open`, and the UI wakes.
pub fn listen(ctx: egui::Context, take: Arc<AtomicBool>, open: Arc<Mutex<Option<PathBuf>>>) {
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else { return };
    let Ok(addr) = listener.local_addr() else { return };
    if std::fs::write(port_file(), addr.port().to_string()).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("instance".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let path = PathBuf::from(line.trim_end_matches(['\r', '\n']));
            let ok = take.load(Ordering::Relaxed) && path.exists();
            let _ = (&stream).write_all(if ok { b"y" } else { b"n" });
            if ok {
                *open.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                ctx.request_repaint();
            }
        }
    });
    drop(spawned);
}
