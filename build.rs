use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let themes_dir = manifest_dir.join("assets/themes");
    println!("cargo:rerun-if-changed={}", themes_dir.display());

    let mut themes = fs::read_dir(&themes_dir)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", themes_dir.display()))
        .map(|entry| {
            entry
                .expect("could not read bundled theme directory entry")
                .path()
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
        })
        .collect::<Vec<_>>();
    themes.sort();
    assert!(
        !themes.is_empty(),
        "assets/themes must contain a TOML theme"
    );

    let mut generated = String::from("&[\n");
    for path in themes {
        println!("cargo:rerun-if-changed={}", path.display());
        generated.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            path.file_name().expect("theme file name").to_string_lossy(),
            path,
        ));
    }
    generated.push_str("]\n");

    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("output directory")).join("bundled_themes.rs");
    fs::write(&output, generated)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", output.display()));
}
