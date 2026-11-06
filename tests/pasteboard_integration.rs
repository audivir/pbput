use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSURLNSPasteboardSupport};
use objc2_foundation::{NSString, NSURL};

// These tests mutate the single shared NSPasteboard.general, so they must not run
// concurrently with each other; this mutex forces sequential execution.
static PASTEBOARD_LOCK: Mutex<()> = Mutex::new(());

fn run_pbput(args: &[&str], stdin: &[u8]) {
    let status = run_pbput_status(args, stdin);
    assert!(status.success(), "pbput exited with {status}");
}

fn run_pbput_status(args: &[&str], stdin: &[u8]) -> std::process::ExitStatus {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pbput"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn pbput");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait().expect("pbput did not exit")
}

fn pasteboard_data(identifier: &str) -> Option<Vec<u8>> {
    let data = NSPasteboard::generalPasteboard().dataForType(&NSString::from_str(identifier));
    data.map(|data| data.to_vec())
}

fn pasteboard_file_url() -> Option<std::path::PathBuf> {
    let pasteboard = NSPasteboard::generalPasteboard();
    let has_file_url = pasteboard.types().is_some_and(|types| {
        types
            .iter()
            .any(|t| &*t == unsafe { NSPasteboardTypeFileURL })
    });
    if !has_file_url {
        return None;
    }
    NSURL::URLFromPasteboard(&pasteboard).and_then(|url| url.to_file_path())
}

#[test]
fn copying_a_file_also_writes_a_file_url_representation() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    let file_path = std::env::temp_dir().join(format!("pbput-test-{}.svg", uuid::Uuid::new_v4()));
    std::fs::write(&file_path, "<svg></svg>").unwrap();

    run_pbput(&[file_path.to_str().unwrap()], b"");

    assert_eq!(pasteboard_file_url().as_deref(), Some(file_path.as_path()));

    std::fs::remove_file(&file_path).ok();
}

#[test]
fn copying_from_stdin_does_not_write_a_file_url_representation() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    run_pbput(&[], b"hello");

    assert_eq!(pasteboard_file_url(), None);
}

#[test]
fn base64_flag_encodes_the_data_before_writing_to_the_pasteboard() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    run_pbput(&["--base64", "--type", "public.utf8-plain-text"], b"hello");

    let pasteboard = NSPasteboard::generalPasteboard();
    let data = pasteboard
        .dataForType(&NSString::from_str("public.utf8-plain-text"))
        .expect("expected data on the pasteboard");
    assert_eq!(data.to_vec(), b"aGVsbG8=");
}

#[test]
fn path_flag_treats_stdin_as_a_path_and_writes_its_file_url_representation() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    let file_path = std::env::temp_dir().join(format!("pbput-test-{}.svg", uuid::Uuid::new_v4()));
    std::fs::write(&file_path, "<svg></svg>").unwrap();

    run_pbput(&["--path"], file_path.to_str().unwrap().as_bytes());

    assert_eq!(pasteboard_file_url().as_deref(), Some(file_path.as_path()));

    std::fs::remove_file(&file_path).ok();
}

#[test]
fn copy_flag_materializes_stdin_and_writes_a_file_url_representation() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    let svg = "<svg></svg>";
    run_pbput(&["--copy", "--type", "public.svg-image"], svg.as_bytes());

    let temp_path = pasteboard_file_url().expect("expected a file URL on the pasteboard");
    assert_eq!(temp_path.extension().unwrap(), "svg");
    assert_eq!(std::fs::read_to_string(&temp_path).unwrap(), svg);

    std::fs::remove_file(&temp_path).ok();
}

#[test]
fn extension_flag_declares_the_type_for_stdin_instead_of_sniffing_a_zip() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    let zip_header = [0x50, 0x4B, 0x03, 0x04, 0, 0];
    run_pbput(&["--extension", "pptx"], &zip_header);

    let pptx = "org.openxmlformats.presentationml.presentation";
    assert_eq!(pasteboard_data(pptx).as_deref(), Some(&zip_header[..]));
    assert_eq!(pasteboard_data("public.zip-archive"), None);
}

#[test]
fn unknown_extension_fails_and_leaves_the_pasteboard_untouched() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    run_pbput(&[], b"previous");
    let status = run_pbput_status(&["--extension", "test.nope-xyz"], b"new");

    assert_eq!(status.code(), Some(2));
    assert_eq!(
        pasteboard_data("public.utf8-plain-text").as_deref(),
        Some(&b"previous"[..])
    );
}

#[test]
fn guess_flag_sniffs_the_content_for_an_unknown_extension() {
    let _guard = PASTEBOARD_LOCK.lock().unwrap();

    let png_header = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
    run_pbput(&["--guess", "--extension", "test.nope-xyz"], &png_header);

    assert_eq!(
        pasteboard_data("public.png").as_deref(),
        Some(&png_header[..])
    );
}
