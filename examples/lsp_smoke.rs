use dios::lsp::LspManager;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let root = std::env::current_dir().unwrap();
    let mgr = LspManager::new();
    eprintln!("[smoke] ensure_session({})", root.display());
    mgr.ensure_session(&root);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last_label = String::new();
    while Instant::now() < deadline {
        let s = mgr.status_for(&root);
        let label = s.label();
        if label != last_label {
            eprintln!("[smoke] status: {label}");
            last_label = label.clone();
        }
        if matches!(s, dios::lsp::LspStatus::Ready) {
            eprintln!("[smoke] READY — handshake worked");
            std::process::exit(0);
        }
        if matches!(s, dios::lsp::LspStatus::Error(_)) {
            eprintln!("[smoke] ERROR — bailing");
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    eprintln!("[smoke] timed out; last status: {last_label}");
    std::process::exit(2);
}
