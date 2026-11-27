#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::session::TICK_LOCK_FILE;

    #[test]
    fn open_creates_the_lock_in_an_existing_directory_and_keeps_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TICK_LOCK_FILE);
        std::fs::write(&path, b"kept").unwrap();
        let file = open(dir.path()).unwrap().expect("the directory exists");
        assert!(still_current(&file, &path));
        assert_eq!(std::fs::read(&path).unwrap(), b"kept");
    }

    #[test]
    fn open_in_a_missing_directory_is_none_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        assert!(open(&gone).unwrap().is_none());
        assert!(!gone.exists());
    }

    #[test]
    fn open_refuses_a_link_planted_at_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(TICK_LOCK_FILE)).unwrap();
        assert!(open(dir.path()).is_err());
        assert!(!elsewhere.exists());
    }

    #[test]
    fn a_lock_whose_file_was_replaced_is_no_longer_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TICK_LOCK_FILE);
        let file = open(dir.path()).unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"").unwrap();
        assert!(!still_current(&file, &path));
    }
}
