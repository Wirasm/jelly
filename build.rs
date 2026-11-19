//! Embeds every file under skill/recipes in the binary, so `jelly recipes` and
//! `jelly new --recipe` work from any directory. skill/recipes stays the source.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("skill/recipes");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut code = String::from("pub static RECIPES: &[Recipe] = &[\n");
    for dir in sorted(&root) {
        if !dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect(&dir, &mut files);
        let name = dir.file_name().unwrap().to_str().unwrap();
        writeln!(code, "    Recipe {{ name: {name:?}, files: &[").unwrap();
        for file in files {
            let rel = file.strip_prefix(&dir).unwrap().components()
                .map(|c| c.as_os_str().to_str().unwrap()).collect::<Vec<_>>().join("/");
            writeln!(code, "        ({rel:?}, include_bytes!({:?})),", file.display().to_string()).unwrap();
        }
        code.push_str("    ] },\n");
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("recipes.rs");
    std::fs::write(out, code).unwrap();
}

/// Entries of a folder by name, without hidden files such as .DS_Store.
fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path())
        .filter(|p| !p.file_name().unwrap().to_string_lossy().starts_with('.'))
        .collect();
    entries.sort();
    entries
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for path in sorted(dir) {
        if path.is_dir() { collect(&path, files) } else { files.push(path) }
    }
}
