//! Integration test for issue #25: crash reports must never contain raw
//! absolute filesystem paths.
//!
//! Unlike `tests/integration.rs`, this test does not raise a fatal signal —
//! it drives `check_previous_crash` end-to-end (parse -> symbolicate ->
//! redact -> write -> archive) against a synthetic, well-formed
//! `last-crash.bin` fixture, so it runs on every platform (Windows and
//! Unix), not just Unix.

use std::path::Path;

/// Capture the *real* current call stack (via `backtrace::trace`, the same
/// underlying mechanism `symbolicate::resolve_frames` uses) so we get
/// genuine, resolvable return addresses. A directly-taken function pointer
/// is not equivalent here: on Windows, taking `some_fn as usize` for a
/// small, unused-except-for-its-address function can resolve to an
/// unrelated symbol (identical-code-folding / nearest-export lookup), so
/// we instead unwind a real stack, which is exactly what a real crash
/// backtrace captures. This test file's own absolute on-disk path is one
/// of the frames that gets resolved this way — on a typical dev machine
/// that path runs through a home directory / username (e.g.
/// `/home/alice/...` or `C:\Users\alice\...`).
fn capture_real_call_stack_ips(max_frames: usize) -> Vec<usize> {
    let mut ips = Vec::with_capacity(max_frames);
    backtrace::trace(|frame| {
        ips.push(frame.ip() as usize);
        ips.len() < max_frames
    });
    ips
}

/// Build a minimal, well-formed `last-crash.bin` blob whose frames are a
/// real, currently-executing call stack, so symbolication resolves to
/// real, absolute, on-disk source paths for this test binary.
fn build_synthetic_crash_blob(app_version: &str) -> Vec<u8> {
    use xai_crash_handler::format::{self, writer};

    let ips = capture_real_call_stack_ips(format::MAX_FRAMES);
    let mut buf = vec![0u8; format::MAX_FILE_SIZE];
    let end = unsafe {
        let mut offset = writer::write_header(
            &mut buf,
            11, // SIGSEGV
            1,  // SEGV_MAPERR
            0xdead_beef,
            std::process::id(),
            1_700_000_000,
            ips.len() as u16,
            app_version.as_bytes(),
        );
        for ip in &ips {
            offset = writer::write_frame(&mut buf, offset, *ip);
        }
        offset
    };
    buf.truncate(end);
    buf
}

#[test]
fn check_previous_crash_never_writes_raw_absolute_path_from_synthetic_fixture() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let blob_bytes = build_synthetic_crash_blob("0.0.0-redaction-fixture");
    std::fs::write(tmp.path().join("last-crash.bin"), &blob_bytes).expect("write synthetic blob");

    // Ground truth: what would raw (unredacted) symbolication actually
    // produce on this machine, for this exact fixture? If backtrace can't
    // resolve a filename for the synthetic frame (e.g. no debug info in
    // this build profile), there's nothing to prove a leak of, so the
    // absolute-path assertions below simply have nothing to check; when it
    // *can* resolve one, we get a real, machine-local absolute path
    // (containing this machine's actual username) to assert against.
    let raw_blob =
        xai_crash_handler::format::CrashBlob::parse(&blob_bytes).expect("blob should parse");
    let raw_frames = xai_crash_handler::symbolicate::resolve_frames(&raw_blob);
    let raw_filenames: Vec<String> = raw_frames
        .iter()
        .filter_map(|f| f.filename.clone())
        .collect();

    let report = xai_crash_handler::check_previous_crash(tmp.path())
        .expect("synthetic fixture should produce a crash report");
    assert_eq!(report.app_version, "0.0.0-redaction-fixture");

    let written = std::fs::read_to_string(&report.report_path).expect("read written report");

    let history_dir = tmp.path().join("history");
    let archived_entries: Vec<_> = std::fs::read_dir(&history_dir)
        .expect("history dir should exist")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        archived_entries.len(),
        1,
        "exactly one archived report expected"
    );
    let archived =
        std::fs::read_to_string(archived_entries[0].path()).expect("read archived report");

    let mut asserted_a_path = false;
    for raw in &raw_filenames {
        let is_absolute = Path::new(raw).is_absolute();
        if !is_absolute {
            continue;
        }
        asserted_a_path = true;
        assert!(
            !written.contains(raw.as_str()),
            "written report leaks raw absolute path {raw:?}:\n{written}"
        );
        assert!(
            !archived.contains(raw.as_str()),
            "archived report leaks raw absolute path {raw:?}:\n{archived}"
        );
    }
    if asserted_a_path {
        eprintln!("verified redaction against a real resolved absolute path from this machine");
    } else {
        eprintln!(
            "warning: backtrace resolved no absolute filename for the synthetic fixture on this \
             build (no debug info?) — redaction of absolute paths was not exercised by this run"
        );
    }

    // The blob and the crash file it came from must still be cleaned up,
    // as with any other processed crash.
    assert!(!tmp.path().join("last-crash.bin").exists());
}
