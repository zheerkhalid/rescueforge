//! End-to-end test: disk images are built in a temp folder, files in them
//! are deleted, found by quick and deep scans, recovered and compared.

#[test]
fn build_delete_scan_recover_compare() {
    match rescueforge::selftest::run() {
        Ok(log) => {
            for line in &log {
                println!("{line}");
            }
            assert!(log.len() >= 13, "every stage ran: {log:#?}");
        }
        Err(e) => panic!("{e}"),
    }
    let leftover =
        std::env::temp_dir().join(format!("rescueforge-selftest-{}", std::process::id()));
    assert!(!leftover.exists());
}
