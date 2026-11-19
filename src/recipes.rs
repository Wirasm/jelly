//! The recipes in skill/recipes, built into the binary by build.rs.

use std::path::Path;

pub struct Recipe {
    pub name: &'static str,
    /// Path inside the recipe folder ("view.html", "assets/card.png") and its bytes.
    pub files: &'static [(&'static str, &'static [u8])],
}

include!(concat!(env!("OUT_DIR"), "/recipes.rs"));

pub fn get(name: &str) -> Result<&'static Recipe, String> {
    RECIPES.iter().find(|r| r.name == name).ok_or_else(|| {
        let names: Vec<&str> = RECIPES.iter().map(|r| r.name).collect();
        format!("no recipe {name:?}; there are {}", names.join(", "))
    })
}

impl Recipe {
    pub fn file(&self, path: &str) -> Option<&'static [u8]> {
        self.files.iter().find(|(p, _)| *p == path).map(|(_, bytes)| *bytes)
    }

    pub fn text(&self, path: &str) -> Option<&'static str> {
        self.file(path).and_then(|b| std::str::from_utf8(b).ok())
    }

    /// The first sentence after "**Use when**" in RECIPE.md, on one line.
    pub fn use_when(&self) -> String {
        let doc = self.text("RECIPE.md").unwrap_or("");
        let Some((_, rest)) = doc.split_once("**Use when**") else { return String::new() };
        let rest = rest.split("\n\n").next().unwrap_or("");
        let line = rest.split_whitespace().collect::<Vec<_>>().join(" ");
        match line.find(". ") {
            Some(end) => line[..=end].to_string(),
            None => line,
        }
    }

    /// The files under assets/, by file name.
    pub fn assets(&self) -> impl Iterator<Item = (&'static str, &'static [u8])> {
        self.files.iter().filter_map(|(p, b)| p.strip_prefix("assets/").map(|f| (f, *b)))
    }

    /// Writes every file into `dir`, keeping shell scripts runnable.
    pub fn write(&self, dir: &Path) -> Result<(), String> {
        for (path, bytes) in self.files {
            let dest = dir.join(path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&dest, bytes).map_err(|e| format!("{}: {e}", dest.display()))?;
            #[cfg(unix)]
            if path.ends_with(".sh") {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("{}: {e}", dest.display()))?;
            }
        }
        Ok(())
    }
}
