// Optionally embeds the desktop app binary (CREATE_PANEL_GUI_BLOB=/path/to/app) so an engine update file
// also updates the app window. Without the variable the blob is empty and nothing is embedded.
use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-env-changed=CREATE_PANEL_GUI_BLOB");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("gui_blob.bin");
    match env::var("CREATE_PANEL_GUI_BLOB") {
        Ok(p) if !p.is_empty() && fs::metadata(&p).is_ok() => { println!("cargo:rerun-if-changed={p}"); fs::copy(&p, &out).unwrap(); }
        Ok(p) if !p.is_empty() => panic!("CREATE_PANEL_GUI_BLOB points to a file that does not exist: {p}"),
        _ => fs::write(&out, b"").unwrap(),
    }
}
