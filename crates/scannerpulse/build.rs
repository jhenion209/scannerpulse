use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    // The frontend's built files are embedded in the binary. A checkout that
    // has not built them embeds one page that says so, so that the Rust
    // build and its tests need no Node.
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
    let dist = web.join("dist");
    // A watched path that does not exist makes Cargo rebuild on every run, so
    // watch the built folder when there is one, else the frontend's folder
    // (creating dist changes it), else only this script.
    if dist.is_dir() {
        println!("cargo:rerun-if-changed={}", dist.display());
    } else if web.is_dir() {
        println!("cargo:rerun-if-changed={}", web.display());
    } else {
        println!("cargo:rerun-if-changed=build.rs");
    }
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("web");
    let wanted = if dist.join("index.html").is_file() {
        read_tree(&dist)
    } else {
        BTreeMap::from([(PathBuf::from("index.html"), NOT_BUILT.as_bytes().to_vec())])
    };
    // Rewrite the copy only when it differs, so that a rerun changes nothing
    // the crate compiles against.
    if read_tree(&out) != wanted {
        let _ = std::fs::remove_dir_all(&out);
        for (path, bytes) in &wanted {
            let target = out.join(path);
            std::fs::create_dir_all(target.parent().expect("a file has a folder"))
                .expect("the build's own folder can be written");
            std::fs::write(target, bytes).expect("the build's own folder can be written");
        }
    }
}

/// Every file under `dir`, by its path relative to it.
fn read_tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("the folder can be read").path();
            if path.is_dir() {
                folders.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let relative = path
                    .strip_prefix(dir)
                    .expect("under the folder")
                    .to_path_buf();
                files.insert(relative, bytes);
            }
        }
    }
    files
}

const NOT_BUILT: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>ScannerPulse</title></head><body><p>ScannerPulse: the web page was not built \
into this binary. Run <code>npm ci &amp;&amp; npm run build</code> in <code>web/</code>, \
then build the binary again.</p></body></html>";
