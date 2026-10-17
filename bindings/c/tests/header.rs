//! `include/loupecam.h` must match what cbindgen generates from the source.
//! Regenerate it with `LOUPECAM_BLESS=1 cargo test -p loupecam-capi --test header`.

#[test]
fn header_is_current() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let config = cbindgen::Config::from_file(format!("{dir}/cbindgen.toml")).unwrap();
    let mut generated = Vec::new();
    cbindgen::Builder::new().with_crate(dir).with_config(config).generate().expect("cbindgen").write(&mut generated);
    let generated = String::from_utf8(generated).unwrap();
    let path = format!("{dir}/include/loupecam.h");
    if std::env::var_os("LOUPECAM_BLESS").is_some() {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    // Compare ignoring line endings (the checkout may have CRLF).
    let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(
        committed == generated,
        "include/loupecam.h is out of date: run `LOUPECAM_BLESS=1 cargo test -p loupecam-capi --test header`"
    );
}
