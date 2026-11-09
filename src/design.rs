//! DESIGN.md (github.com/google-labs-code/design.md): a project's design
//! tokens in YAML front matter, with the reasoning in markdown below. jelly
//! turns the tokens into CSS variables any view in the project can use.

/// Flattens the front matter's nested keys into `--a-b-c: value;` lines.
/// Handles the token subset of YAML: nested maps by indentation and scalar
/// values. Lists and anything fancier are skipped.
pub fn css_vars(md: &str) -> String {
    let Some(front) = md.strip_prefix("---").and_then(|r| r.split_once("\n---")).map(|(f, _)| f) else { return String::new() };
    let mut path: Vec<(usize, String)> = Vec::new();
    let mut out = String::new();
    for raw in front.lines() {
        let line = raw.trim_end();
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') || body.starts_with('-') {
            continue;
        }
        let indent = line.len() - body.len();
        let Some((key, value)) = body.split_once(':') else { continue };
        while path.last().is_some_and(|(i, _)| *i >= indent) {
            path.pop();
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        let key = kebab(key.trim().trim_matches('"'));
        if value.is_empty() {
            path.push((indent, key));
            continue;
        }
        if path.is_empty() {
            continue; // top-level scalars such as name and version are not tokens
        }
        let name = path.iter().map(|(_, k)| k.as_str()).chain([key.as_str()]).collect::<Vec<_>>().join("-");
        let name = name.replacen("colors-", "color-", 1);
        out.push_str(&format!("  --{name}: {value};\n"));
    }
    out
}

fn kebab(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii_alphanumeric() || c == '-' {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn tokens_become_css_variables() {
        let md = "---\nname: Heritage\ncolors:\n  primary: \"#1A1C1E\"\n  tertiary: \"#B8422E\"\ntypography:\n  h1:\n    fontFamily: Public Sans\n    fontSize: 3rem\nrounded:\n  md: 8px\n---\n\n## Overview\n";
        let css = super::css_vars(md);
        assert!(css.contains("--color-primary: #1A1C1E;"));
        assert!(css.contains("--typography-h1-font-family: Public Sans;"));
        assert!(css.contains("--rounded-md: 8px;"));
        assert!(!css.contains("name"));
    }
}
