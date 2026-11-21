// Throwaway: process-isolation probe (never merged). One test sets a
// process environment variable, the other asserts it is unset; both pass
// only when each test runs in its own process.
#[path = "support/test_home.rs"]
mod test_home;

const VAR: &str = "KOTO_THROWAWAY_ISOLATION_PROBE_7F3A";

#[test]
fn throwaway_env_sets_variable() {
    std::env::set_var(VAR, "set");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(std::env::var(VAR).as_deref(), Ok("set"));
}

#[test]
fn throwaway_env_variable_is_unset() {
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert!(std::env::var(VAR).is_err());
}
