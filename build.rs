//! Embeds every file under skill/recipes in the binary, so `jelly recipes` and
//! `jelly new --recipe` work from any directory. skill/recipes stays the source.
//! Also works out the version the binary reports, as `JELLY_VERSION`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    version();
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

/// The version is the release's, not the one in Cargo.toml: a release sets
/// `JELLY_VERSION`, and a build from a checkout takes it from the latest release
/// tag. `v0.1.4` is 0.1.4, and three commits later 0.1.4+3 (still 0.1.4 when
/// compared with a release). Without git, Cargo.toml's version.
fn version() {
    println!("cargo:rerun-if-env-changed=JELLY_VERSION");
    let version = std::env::var("JELLY_VERSION").ok().filter(|v| !v.is_empty())
        .or_else(from_tag)
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=JELLY_VERSION={version}");
}

fn from_tag() -> Option<String> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    // Describe again when HEAD moves or tags arrive.
    let mut watch = vec!["HEAD", "packed-refs", "refs/tags"].into_iter().map(String::from).collect::<Vec<_>>();
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) { watch.push(branch); }
    for path in watch {
        if let Some(p) = git(&["rev-parse", "--path-format=absolute", "--git-path", &path]) {
            if Path::new(&p).exists() { println!("cargo:rerun-if-changed={p}"); }
        }
    }
    // v0.1.4-3-gabc1234: the tag, the commits since it, the commit.
    let described = git(&["describe", "--tags", "--long", "--match", "v[0-9]*"])?;
    let mut parts = described.rsplitn(3, '-');
    let (_commit, since, tag) = (parts.next()?, parts.next()?, parts.next()?);
    let tag = tag.strip_prefix('v')?;
    Some(if since == "0" { tag.to_string() } else { format!("{tag}+{since}") })
}
