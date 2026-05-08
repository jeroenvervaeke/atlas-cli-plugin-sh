use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::Path;

type BuildResult = Result<(), Box<dyn Error>>;

fn main() -> BuildResult {
    println!("cargo:rerun-if-changed=manifest.template.yml");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let vars = build_template_vars()?;
    generate_manifest(&vars)?;
    Ok(())
}

fn build_template_vars() -> Result<HashMap<&'static str, String>, Box<dyn Error>> {
    let version = env::var("CARGO_PKG_VERSION")?;

    let binary = env::var("CARGO_BIN_NAME")
        .or_else(|_| env::var("CARGO_PKG_NAME"))
        .map_err(|_| "Failed to read binary name from CARGO_BIN_NAME or CARGO_PKG_NAME")?;

    // Add .exe extension on Windows.
    let binary = if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        format!("{binary}.exe")
    } else {
        binary
    };

    let repo_url = env::var("CARGO_PKG_REPOSITORY")?;
    let github_path = repo_url
        .strip_prefix("https://github.com/")
        .ok_or("Repository URL must start with 'https://github.com/'")?;
    let (owner, name) = github_path
        .split_once('/')
        .ok_or("Repository URL must be in format 'owner/name'")?;

    Ok(HashMap::from([
        ("VERSION", version),
        ("BINARY", binary),
        ("GITHUB_REPOSITORY_OWNER", owner.to_string()),
        ("GITHUB_REPOSITORY_NAME", name.to_string()),
    ]))
}

fn generate_manifest(vars: &HashMap<&str, String>) -> BuildResult {
    let template_path = Path::new("manifest.template.yml");
    let output_path = Path::new("manifest.yml");

    let template_content = fs::read_to_string(template_path)?;

    // Anchored `${VAR}` substitution avoids `$VERSION` matching inside
    // `$VERSION_OLD` if the template gains overlapping variable names.
    let result = vars.iter().fold(template_content, |content, (var, value)| {
        content.replace(&format!("${{{var}}}"), value)
    });

    fs::write(output_path, result)?;

    for (var, value) in vars {
        println!("cargo:warning=Using {var}={value}");
    }
    Ok(())
}
