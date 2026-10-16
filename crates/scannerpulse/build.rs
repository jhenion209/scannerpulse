use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    // The frontend's built files are embedded in the binary. A checkout that
    // has not built them embeds one page that says so, so that the Rust
    // build and its tests need no Node.
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("web");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).expect("the build's own folder can be written");
    if dist.join("index.html").is_file() {
        copy(&dist, &out);
    } else {
        std::fs::write(out.join("index.html"), NOT_BUILT)
            .expect("the build's own folder can be written");
    }
}

fn copy(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).expect("web/dist can be read") {
        let entry = entry.expect("web/dist can be read");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("web/dist can be read").is_dir() {
            std::fs::create_dir_all(&target).expect("the build's own folder can be written");
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("the build's own folder can be written");
        }
    }
}

const NOT_BUILT: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>ScannerPulse</title></head><body><p>ScannerPulse: the web page was not built \
into this binary. Run <code>npm ci &amp;&amp; npm run build</code> in <code>web/</code>, \
then build the binary again.</p></body></html>";
