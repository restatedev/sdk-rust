#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
    #[cfg(not(feature = "reqwest-client"))]
    t.compile_fail("tests/ui/reqwest-client/*.rs");
    #[cfg(feature = "reqwest-client")]
    t.pass("tests/ui/reqwest-client/*.rs");
}
