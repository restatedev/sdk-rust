#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
    #[cfg(not(feature = "ingress-client"))]
    t.compile_fail("tests/ui/ingress-client/*.rs");
    #[cfg(feature = "ingress-client")]
    t.pass("tests/ui/ingress-client/*.rs");
}
