//! Body templates: scaffolds for new tickets, embedded at compile time and seeded
//! into a repo on `init`. A template is a markdown body with `{{title}}` / `{{id}}`
//! placeholders; `create --template <name>` loads `.ticketsplease/templates/<name>.md`
//! and substitutes them. The bundled examples teach the house body convention
//! (Goal / Gap / Work / Acceptance / Refs); a repo can add or edit its own.

use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use ticketsplease_core::{Error, Result};

/// The `templates/` directory, baked into the binary so `init` can seed examples
/// even in an offline/fresh repo.
static TEMPLATES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// Directory holding a repo's body templates, relative to the repo root.
const TEMPLATES_SUBDIR: &str = ".ticketsplease/templates";

/// Seed the bundled example templates into `<repo>/.ticketsplease/templates/`.
/// `extract` overwrites existing files of the same name, refreshing the examples to
/// the current version while leaving a repo's own templates untouched. Returns the path.
pub fn install(repo: &Path) -> Result<PathBuf> {
    let target = repo.join(TEMPLATES_SUBDIR);
    std::fs::create_dir_all(&target).map_err(Error::Io)?;
    TEMPLATES_DIR.extract(&target).map_err(Error::Io)?;
    Ok(target)
}

/// Load a template body by name from `<repo>/.ticketsplease/templates/<name>.md`,
/// with `{{title}}` / `{{id}}` substituted. `NotFound` (exit 4) if absent.
pub fn load(repo: &Path, name: &str, id: &str, title: &str) -> Result<String> {
    let path = repo.join(TEMPLATES_SUBDIR).join(format!("{name}.md"));
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::NotFound(format!("template `{name}` ({})", path.display()))
        } else {
            Error::Invalid(format!("cannot read template {}: {e}", path.display()))
        }
    })?;
    Ok(substitute(&raw, id, title))
}

/// Replace `{{title}}` / `{{id}}` placeholders in a template body.
fn substitute(text: &str, id: &str, title: &str) -> String {
    text.replace("{{title}}", title).replace("{{id}}", id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_replaces_placeholders() {
        let out = substitute("# {{title}}\n<!-- {{id}} -->\n", "my-id", "My Title");
        assert_eq!(out, "# My Title\n<!-- my-id -->\n");
    }

    #[test]
    fn bundled_examples_are_embedded() {
        // The examples ship in the binary so `init` can seed them offline.
        assert!(TEMPLATES_DIR.get_file("default.md").is_some());
        assert!(TEMPLATES_DIR.get_file("audit.md").is_some());
    }
}
