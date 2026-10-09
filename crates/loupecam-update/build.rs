fn main() { println!("cargo:rustc-env=LOUPECAM_TARGET={}", std::env::var("TARGET").unwrap()); }
