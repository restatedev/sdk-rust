// One UI fixture deliberately instantiates `HttpServer`; run this suite only
// when that public surface is enabled. Minimal tunnel-only test matrices still
// exercise all tunnel unit and integration tests.
#[cfg(feature = "http_server")]
#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
