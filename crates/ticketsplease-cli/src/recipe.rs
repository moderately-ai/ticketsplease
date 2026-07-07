//! The recipe runner: execute a named, typed, parameterized procedure over the tool's
//! own subcommands (`tkt run <name>`). A recipe declares typed `inputs`, an ordered list
//! of `steps` (each a structured invocation of an existing subcommand), and optional
//! `outputs`. Inputs are validated *before* any step runs; each step is executed by
//! re-invoking this same binary with `--format json` so it inherits every command's
//! validation, exit-code contract, and JSON output; the failing step's exit code
//! propagates. The only cross-step data flow is `{{steps.<id>.<dotted.path>}}` — a scalar
//! pulled from a prior step's JSON, which hard-fails if the path is absent (deliberately
//! unlike a silent empty string).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use include_dir::{include_dir, Dir};
use serde_json::Value;
use ticketsplease_core::config::{InputType, RecipeInput};
use ticketsplease_core::{Error, Result};

/// Keys of a step table that are not `--flags`.
const RESERVED_KEYS: [&str; 3] = ["command", "args", "id"];

/// The bundled example recipes, baked into the binary so `init` can seed them offline.
static RECIPES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/recipes");

/// Directory holding a repo's discovered recipes, relative to the repo root.
const RECIPES_SUBDIR: &str = ".ticketsplease/recipes";

/// Seed the bundled example recipes into `<repo>/.ticketsplease/recipes/`. `extract`
/// overwrites files of the same name (refreshing the examples) while leaving a repo's
/// own differently-named recipes untouched — so customize by copying to a new name.
pub fn install(repo: &Path) -> Result<PathBuf> {
    let target = repo.join(RECIPES_SUBDIR);
    std::fs::create_dir_all(&target).map_err(Error::Io)?;
    RECIPES_DIR.extract(&target).map_err(Error::Io)?;
    Ok(target)
}

/// Values available to `{{...}}` substitution while a recipe runs.
pub struct RunContext {
    /// Resolved input values (a `multiple` input stays comma-joined).
    pub inputs: BTreeMap<String, String>,
    /// Captured JSON output of each completed step, keyed by its `id`.
    pub steps: BTreeMap<String, Value>,
}

impl RunContext {
    #[must_use]
    pub fn new(inputs: BTreeMap<String, String>) -> Self {
        Self {
            inputs,
            steps: BTreeMap::new(),
        }
    }
}

/// Parse `--arg key=value` occurrences into a map.
pub fn parse_args(args: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for a in args {
        let (k, v) = a
            .split_once('=')
            .ok_or_else(|| Error::Invalid(format!("--arg must be `key=value`, got `{a}`")))?;
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

/// Validate the provided `--arg` values against the recipe's declared inputs, returning
/// the resolved value map. Runs before any step, so a bad input aborts with nothing
/// mutated: a missing required input / wrong type / bad enum is `Invalid` (exit 3); a
/// `ticket` input that does not resolve is `NotFound` (exit 4).
pub fn validate_inputs(
    defs: &BTreeMap<String, RecipeInput>,
    provided: &BTreeMap<String, String>,
    ticket_ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>> {
    for k in provided.keys() {
        if !defs.contains_key(k) {
            return Err(Error::Invalid(format!(
                "unknown input `{k}` (not declared by this recipe)"
            )));
        }
    }
    let mut out = BTreeMap::new();
    for (name, def) in defs {
        let raw = match provided.get(name).cloned().or_else(|| def.default.clone()) {
            Some(v) => v,
            None => {
                if def.required {
                    return Err(Error::Invalid(format!("missing required input `{name}`")));
                }
                continue;
            }
        };
        // A `multiple` input is a comma-separated list; validate each element.
        let values: Vec<&str> = if def.multiple {
            raw.split(',').map(str::trim).collect()
        } else {
            vec![raw.as_str()]
        };
        for val in values {
            validate_value(name, def, val, ticket_ids)?;
        }
        out.insert(name.clone(), raw);
    }
    Ok(out)
}

fn validate_value(
    name: &str,
    def: &RecipeInput,
    val: &str,
    ticket_ids: &BTreeSet<String>,
) -> Result<()> {
    match def.input_type {
        InputType::String => Ok(()),
        InputType::Int => val
            .parse::<i64>()
            .map(|_| ())
            .map_err(|_| Error::Invalid(format!("input `{name}` must be an integer, got `{val}`"))),
        InputType::Bool => {
            if val == "true" || val == "false" {
                Ok(())
            } else {
                Err(Error::Invalid(format!(
                    "input `{name}` must be true or false, got `{val}`"
                )))
            }
        }
        InputType::Enum => {
            if def.options.iter().any(|o| o == val) {
                Ok(())
            } else {
                Err(Error::Invalid(format!(
                    "input `{name}` must be one of [{}], got `{val}`",
                    def.options.join(", ")
                )))
            }
        }
        InputType::Ticket => {
            if ticket_ids.contains(val) {
                Ok(())
            } else {
                Err(Error::NotFound(format!(
                    "input `{name}`: ticket `{val}` does not exist"
                )))
            }
        }
    }
}

/// Build a step's argv (`[command, positional…, --flag, value…]`) from its table,
/// substituting `{{...}}` templates. With `dry`, `{{steps.*}}` references are left
/// literal (steps have not run yet); otherwise they resolve against `ctx.steps`.
pub fn build_argv(step: &toml::Table, ctx: &RunContext, dry: bool) -> Result<Vec<String>> {
    let command = step
        .get("command")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| Error::Invalid("recipe step is missing a `command`".into()))?;
    let mut argv = vec![command.to_string()];

    if let Some(args) = step.get("args") {
        let arr = args
            .as_array()
            .ok_or_else(|| Error::Invalid(format!("step `{command}`: `args` must be an array")))?;
        for a in arr {
            let s = a.as_str().ok_or_else(|| {
                Error::Invalid(format!("step `{command}`: `args` entries must be strings"))
            })?;
            argv.push(resolve_template(s, ctx, dry)?);
        }
    }

    // Every remaining key is a `--flag`. `toml::Table` iterates in sorted key order,
    // which is fine — clap does not care about flag order.
    for (key, value) in step {
        if RESERVED_KEYS.contains(&key.as_str()) {
            continue;
        }
        let flag = format!("--{key}");
        match value {
            toml::Value::String(s) => {
                argv.push(flag);
                argv.push(resolve_template(s, ctx, dry)?);
            }
            toml::Value::Boolean(true) => argv.push(flag),
            toml::Value::Boolean(false) => {}
            toml::Value::Integer(n) => {
                argv.push(flag);
                argv.push(n.to_string());
            }
            toml::Value::Array(arr) => {
                for elem in arr {
                    let s = elem.as_str().ok_or_else(|| {
                        Error::Invalid(format!(
                            "step `{command}`: flag `{key}` list entries must be strings"
                        ))
                    })?;
                    argv.push(flag.clone());
                    argv.push(resolve_template(s, ctx, dry)?);
                }
            }
            _ => {
                return Err(Error::Invalid(format!(
                    "step `{command}`: flag `{key}` has an unsupported value type"
                )))
            }
        }
    }
    Ok(argv)
}

/// Execute one step by re-invoking this binary with `--format json`, returning its parsed
/// JSON output. A non-zero step exit propagates as the matching error (so the recipe exits
/// with the failing step's code); the child already printed its own error to stderr.
pub fn exec_step(repo: &Path, argv: &[String], label: &str) -> Result<Value> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Internal(format!("cannot locate the ticketsplease binary: {e}")))?;
    let output = Command::new(&exe)
        .arg("--repo")
        .arg(repo)
        .args(["--format", "json"])
        .args(argv)
        .output()
        .map_err(|e| Error::Internal(format!("running step {label}: {e}")))?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(error_for_code(
            code,
            format!("recipe step {label} failed: {}", stderr.trim()),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .map_err(|e| Error::Internal(format!("step {label} did not emit JSON: {e}")))
}

/// Map a child process exit code back to the matching `Error` so the recipe exits with
/// the failing step's code (the stable exit-code contract).
fn error_for_code(code: i32, msg: String) -> Error {
    match code {
        3 => Error::Invalid(msg),
        4 => Error::NotFound(msg),
        5 => Error::Cycle(msg),
        6 => Error::Conflict(msg),
        7 => Error::Timeout(msg),
        _ => Error::Internal(msg),
    }
}

/// Substitute `{{inputs.<name>}}` and `{{steps.<id>.<dotted.path>}}` in a template. With
/// `dry`, step references are left literal (they have not run). A referenced-but-unknown
/// input, or a step path that does not resolve to a scalar, is a hard `Invalid` error.
pub fn resolve_template(tpl: &str, ctx: &RunContext, dry: bool) -> Result<String> {
    let mut out = String::new();
    let mut rest = tpl;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| Error::Invalid(format!("unterminated `{{{{` in `{tpl}`")))?;
        let var = after[..end].trim();
        out.push_str(&resolve_var(var, ctx, dry)?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve_var(var: &str, ctx: &RunContext, dry: bool) -> Result<String> {
    if let Some(name) = var.strip_prefix("inputs.") {
        ctx.inputs
            .get(name)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("recipe references undeclared input `{name}`")))
    } else if let Some(rest) = var.strip_prefix("steps.") {
        if dry {
            // Steps have not executed yet — keep the reference symbolic in the plan.
            return Ok(format!("{{{{{var}}}}}"));
        }
        let (id, path) = rest.split_once('.').ok_or_else(|| {
            Error::Invalid(format!(
                "bad step reference `{{{{{var}}}}}` (expected steps.<id>.<key>)"
            ))
        })?;
        let json = ctx.steps.get(id).ok_or_else(|| {
            Error::Invalid(format!(
                "recipe references step `{id}` before it has run (a step id is only \
                 available to later steps)"
            ))
        })?;
        let value = resolve_path(json, path)
            .ok_or_else(|| Error::Invalid(format!("step `{id}` output has no path `{path}`")))?;
        scalar_to_string(value).ok_or_else(|| {
            Error::Invalid(format!("step `{id}` path `{path}` is not a scalar value"))
        })
    } else {
        Err(Error::Invalid(format!(
            "unknown template variable `{{{{{var}}}}}` (use inputs.* or steps.<id>.*)"
        )))
    }
}

/// Walk a dotted path (object keys and numeric array indices) into a JSON value.
fn resolve_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = value;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(map) => map.get(seg)?,
            Value::Array(arr) => arr.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// A JSON scalar as a string; `None` for null/array/object.
fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RunContext {
        let mut c = RunContext::new(BTreeMap::from([("id".into(), "auth".into())]));
        c.steps.insert(
            "piece".into(),
            serde_json::json!({ "results": [ { "id": "auth-api" } ] }),
        );
        c
    }

    #[test]
    fn substitutes_inputs_and_step_paths() {
        let c = ctx();
        assert_eq!(
            resolve_template("x-{{inputs.id}}", &c, false).unwrap(),
            "x-auth"
        );
        assert_eq!(
            resolve_template("{{steps.piece.results.0.id}}", &c, false).unwrap(),
            "auth-api"
        );
    }

    #[test]
    fn dry_run_keeps_step_refs_symbolic() {
        let c = ctx();
        assert_eq!(
            resolve_template("{{steps.piece.results.0.id}}", &c, true).unwrap(),
            "{{steps.piece.results.0.id}}"
        );
    }

    #[test]
    fn missing_step_path_hard_fails() {
        let c = ctx();
        assert!(resolve_template("{{steps.piece.results.9.id}}", &c, false).is_err());
        assert!(resolve_template("{{inputs.nope}}", &c, false).is_err());
    }

    #[test]
    fn build_argv_positionals_then_flags() {
        let c = ctx();
        let step: toml::Table =
            toml::from_str("command = \"set\"\nargs = [\"{{inputs.id}}\"]\nadd-related = \"x\"\n")
                .unwrap();
        assert_eq!(
            build_argv(&step, &c, false).unwrap(),
            vec!["set", "auth", "--add-related", "x"]
        );
    }

    #[test]
    fn build_argv_bool_flag_and_where() {
        let c = ctx();
        let step: toml::Table =
            toml::from_str("command = \"set\"\nwhere = \"dep:{{inputs.id}}\"\nforce = true\n")
                .unwrap();
        // Sorted keys: force before where.
        assert_eq!(
            build_argv(&step, &c, false).unwrap(),
            vec!["set", "--force", "--where", "dep:auth"]
        );
    }
}
