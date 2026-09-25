//! Guard: `cargo test` must run with the decider forced off.
//!
//! `.cargo/config.toml` sets `KOTO_DECIDER = { value = "off", force = true }`
//! so a developer's exported `KOTO_DECIDER` (and key) can't make a test
//! consult a real decider. If this test fails, that isolation is gone.

#[test]
fn koto_decider_is_forced_off_for_tests() {
    let got = std::env::var("KOTO_DECIDER");
    assert_eq!(
        got.as_deref(),
        Ok("off"),
        "KOTO_DECIDER must be \"off\" inside cargo test, got {:?}. \
         Check that .cargo/config.toml still has \
         `[env] KOTO_DECIDER = {{ value = \"off\", force = true }}`; \
         without it an exported KOTO_DECIDER could reach a real decider.",
        got
    );
}
