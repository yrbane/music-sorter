use std::process::Command;

#[test]
fn test_help_flag() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .output()
        .expect("Impossible de lancer music-sorter");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("music-sorter"));
    assert!(stdout.contains("--source"));
    assert!(stdout.contains("--target"));
    assert!(stdout.contains("--workers"));
    assert!(stdout.contains("--move"));
}

#[test]
fn test_empty_source_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = tempfile::TempDir::new().unwrap();

    let output = Command::new("cargo")
        .args([
            "run", "--", "--source", dir.path().to_str().unwrap(),
            "--target", target.path().to_str().unwrap(),
        ])
        .output()
        .expect("Impossible de lancer music-sorter");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0 fichiers audio"));
}

#[test]
fn test_nonexistent_source() {
    let output = Command::new("cargo")
        .args(["run", "--", "--source", "/tmp/nonexistent_music_sorter_test_xyz"])
        .output()
        .expect("Impossible de lancer music-sorter");

    assert!(!output.status.success());
}
