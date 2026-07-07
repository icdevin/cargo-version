use clap::Parser;
use semver::{BuildMetadata, Prerelease, Version};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use toml_edit::{DocumentMut, value};

const MANIFEST: &str = "Cargo.toml";
const LOCKFILE: &str = "Cargo.lock";

#[derive(Parser)]
#[command(
    name = "cargo-version",
    version,
    about = "Bump a Cargo package version, commit the change, and tag it",
    long_about = "Bump the current package version in Cargo.toml, update Cargo.lock when present, then create an X.Y.Z git commit and vX.Y.Z tag when run inside a git worktree."
)]
struct Cli {
    #[arg(
        value_name = "VERSION",
        value_parser = parse_version_request,
        help = "Version to set or component to increment",
        long_help = "Version to set or component to increment: major, minor, patch, premajor, preminor, prepatch, prerelease, or an exact semver version"
    )]
    request: VersionRequest,

    #[arg(
        long = "no-git-tag-version",
        help = "Update Cargo files without creating a git commit or tag"
    )]
    no_git_tag_version: bool,

    #[arg(
        short = 'm',
        long = "message",
        default_value = "%s",
        help = "Commit message to use, with %s replaced by the new version"
    )]
    message: String,

    #[arg(
        long = "sign-git-tag",
        help = "Create a signed git tag with git tag -s"
    )]
    sign_git_tag: bool,

    #[arg(
        long = "preid",
        value_name = "IDENTIFIER",
        help = "Prerelease identifier for prerelease bumps, such as rc"
    )]
    preid: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum VersionRequest {
    Bump(Bump),
    Exact(Version),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Bump {
    Major,
    Minor,
    Patch,
    Premajor,
    Preminor,
    Prepatch,
    Prerelease,
}

#[derive(Debug, Eq, PartialEq)]
struct ManifestUpdate {
    content: String,
    package_name: String,
    old_version: Version,
    new_version: Version,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cargo-version: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    let manifest_path = Path::new(MANIFEST);
    let manifest = fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read {MANIFEST}: {error}"))?;
    let update = update_manifest_version(&manifest, &cli.request, cli.preid.as_deref())?;
    let lockfile_update = lockfile_update_if_present(&update)?;
    let git = GitState::discover()?;
    let should_commit_and_tag = git.in_worktree && !cli.no_git_tag_version;

    if should_commit_and_tag {
        ensure_git_clean()?;
    }

    fs::write(manifest_path, update.content)
        .map_err(|error| format!("failed to write {MANIFEST}: {error}"))?;

    if let Some((path, content)) = &lockfile_update {
        fs::write(path, content)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }

    if should_commit_and_tag {
        let mut paths = vec![PathBuf::from(MANIFEST)];
        if let Some((path, _)) = &lockfile_update {
            paths.push(path.clone());
        }
        commit_and_tag(&paths, &update.new_version, &cli.message, cli.sign_git_tag)?;
    }

    println!("v{}", update.new_version);
    Ok(())
}

fn parse_version_request(value: &str) -> Result<VersionRequest, String> {
    match value {
        "major" => Ok(VersionRequest::Bump(Bump::Major)),
        "minor" => Ok(VersionRequest::Bump(Bump::Minor)),
        "patch" => Ok(VersionRequest::Bump(Bump::Patch)),
        "premajor" => Ok(VersionRequest::Bump(Bump::Premajor)),
        "preminor" => Ok(VersionRequest::Bump(Bump::Preminor)),
        "prepatch" => Ok(VersionRequest::Bump(Bump::Prepatch)),
        "prerelease" => Ok(VersionRequest::Bump(Bump::Prerelease)),
        _ => Version::parse(value)
            .map(VersionRequest::Exact)
            .map_err(|error| format!("invalid version '{value}': {error}")),
    }
}

fn update_manifest_version(
    manifest: &str,
    request: &VersionRequest,
    preid: Option<&str>,
) -> Result<ManifestUpdate, String> {
    let mut doc = manifest
        .parse::<DocumentMut>()
        .map_err(|error| format!("failed to parse {MANIFEST}: {error}"))?;

    let package = doc["package"]
        .as_table_mut()
        .ok_or_else(|| String::from("could not find [package] in Cargo.toml"))?;

    let package_name = package["name"]
        .as_str()
        .ok_or_else(|| String::from("could not find package.name in Cargo.toml"))?
        .to_string();

    let old_version = package["version"]
        .as_str()
        .ok_or_else(|| String::from("could not find package.version in Cargo.toml"))?;
    let old_version = Version::parse(old_version)
        .map_err(|error| format!("unsupported package.version '{old_version}': {error}"))?;
    let new_version = next_version(&old_version, request, preid)?;

    if new_version == old_version {
        return Err(format!("version is already {new_version}"));
    }

    package["version"] = value(new_version.to_string());

    Ok(ManifestUpdate {
        content: doc.to_string(),
        package_name,
        old_version,
        new_version,
    })
}

fn next_version(
    version: &Version,
    request: &VersionRequest,
    preid: Option<&str>,
) -> Result<Version, String> {
    validate_preid(preid)?;

    match request {
        VersionRequest::Exact(version) => Ok(version.clone()),
        VersionRequest::Bump(bump) => bumped_version(version, *bump, preid),
    }
}

fn validate_preid(preid: Option<&str>) -> Result<(), String> {
    if let Some(preid) = preid {
        if preid.is_empty() {
            return Err(String::from("--preid cannot be empty"));
        }

        Prerelease::new(&format!("{preid}.0"))
            .map_err(|error| format!("invalid --preid '{preid}': {error}"))?;
    }

    Ok(())
}

fn bumped_version(version: &Version, bump: Bump, preid: Option<&str>) -> Result<Version, String> {
    let mut next = version.clone();

    match bump {
        Bump::Major => {
            next.major += 1;
            next.minor = 0;
            next.patch = 0;
        }
        Bump::Minor => {
            next.minor += 1;
            next.patch = 0;
        }
        Bump::Patch => {
            if next.pre.is_empty() {
                next.patch += 1;
            }
        }
        Bump::Premajor => {
            next.major += 1;
            next.minor = 0;
            next.patch = 0;
            next.pre = initial_prerelease(preid)?;
            next.build = BuildMetadata::EMPTY;
            return Ok(next);
        }
        Bump::Preminor => {
            next.minor += 1;
            next.patch = 0;
            next.pre = initial_prerelease(preid)?;
            next.build = BuildMetadata::EMPTY;
            return Ok(next);
        }
        Bump::Prepatch => {
            next.patch += 1;
            next.pre = initial_prerelease(preid)?;
            next.build = BuildMetadata::EMPTY;
            return Ok(next);
        }
        Bump::Prerelease => {
            if next.pre.is_empty() {
                next.patch += 1;
                next.pre = initial_prerelease(preid)?;
            } else {
                next.pre = increment_prerelease(&next.pre, preid)?;
            }
            next.build = BuildMetadata::EMPTY;
            return Ok(next);
        }
    }

    next.pre = Prerelease::EMPTY;
    next.build = BuildMetadata::EMPTY;
    Ok(next)
}

fn initial_prerelease(preid: Option<&str>) -> Result<Prerelease, String> {
    let value = match preid {
        Some(preid) => format!("{preid}.0"),
        None => String::from("0"),
    };

    Prerelease::new(&value).map_err(|error| format!("invalid prerelease '{value}': {error}"))
}

fn increment_prerelease(current: &Prerelease, preid: Option<&str>) -> Result<Prerelease, String> {
    let current = current.as_str();
    let mut parts = current.split('.').collect::<Vec<_>>();

    if let Some(preid) = preid {
        let preid_parts = preid.split('.').collect::<Vec<_>>();
        let prefix_matches =
            parts.len() > preid_parts.len() && parts[..preid_parts.len()] == preid_parts;

        if !prefix_matches {
            return initial_prerelease(Some(preid));
        }
    }

    if let Some(last) = parts.last_mut() {
        if let Ok(value) = last.parse::<u64>() {
            let incremented = value + 1;
            let mut updated = parts[..parts.len() - 1].join(".");
            if !updated.is_empty() {
                updated.push('.');
            }
            updated.push_str(&incremented.to_string());
            return Prerelease::new(&updated)
                .map_err(|error| format!("invalid prerelease '{updated}': {error}"));
        }
    }

    let updated = format!("{current}.0");
    Prerelease::new(&updated).map_err(|error| format!("invalid prerelease '{updated}': {error}"))
}

fn lockfile_update_if_present(
    update: &ManifestUpdate,
) -> Result<Option<(PathBuf, String)>, String> {
    let Some(path) = lockfile_path()? else {
        return Ok(None);
    };

    let lockfile = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let content = update_lockfile_package(
        &lockfile,
        &update.package_name,
        &update.old_version,
        &update.new_version,
    )?;

    Ok(Some((path, content)))
}

fn lockfile_path() -> Result<Option<PathBuf>, String> {
    let local = PathBuf::from(LOCKFILE);
    if local.exists() {
        return Ok(Some(local));
    }

    let output = match Command::new("cargo")
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    if !output.status.success() {
        return Ok(None);
    }

    let manifest = String::from_utf8(output.stdout)
        .map_err(|_| String::from("cargo locate-project returned invalid UTF-8"))?;
    let manifest = manifest.trim();
    if manifest.is_empty() {
        return Ok(None);
    }

    let Some(workspace_root) = Path::new(manifest).parent() else {
        return Ok(None);
    };
    let lockfile = workspace_root.join(LOCKFILE);

    Ok(lockfile.exists().then_some(lockfile))
}

fn update_lockfile_package(
    lockfile: &str,
    package_name: &str,
    old_version: &Version,
    new_version: &Version,
) -> Result<String, String> {
    let mut doc = lockfile
        .parse::<DocumentMut>()
        .map_err(|error| format!("failed to parse {LOCKFILE}: {error}"))?;
    let packages = doc["package"]
        .as_array_of_tables_mut()
        .ok_or_else(|| String::from("could not find [[package]] entries in Cargo.lock"))?;

    let old_version = old_version.to_string();
    let mut updated = false;

    for package in packages.iter_mut() {
        let name_matches = package["name"].as_str() == Some(package_name);
        let version_matches = package["version"].as_str() == Some(old_version.as_str());

        if name_matches && version_matches {
            package["version"] = value(new_version.to_string());
            updated = true;
        }
    }

    if !updated {
        return Err(format!(
            "could not find {package_name} {old_version} in {LOCKFILE}"
        ));
    }

    Ok(doc.to_string())
}

#[derive(Debug)]
struct GitState {
    in_worktree: bool,
}

impl GitState {
    fn discover() -> Result<Self, String> {
        let output = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .output();

        let Ok(output) = output else {
            return Ok(Self { in_worktree: false });
        };

        if !output.status.success() {
            return Ok(Self { in_worktree: false });
        }

        let stdout = String::from_utf8(output.stdout)
            .map_err(|_| String::from("git returned invalid UTF-8"))?;

        Ok(Self {
            in_worktree: stdout.trim() == "true",
        })
    }
}

fn ensure_git_clean() -> Result<(), String> {
    let output = git_output(["status", "--porcelain", "--untracked-files=no"])?;

    if !output.trim().is_empty() {
        return Err(String::from(
            "git working tree has tracked changes. Commit or stash them before running cargo-version",
        ));
    }

    Ok(())
}

fn commit_and_tag(
    paths: &[PathBuf],
    version: &Version,
    message_template: &str,
    sign_git_tag: bool,
) -> Result<(), String> {
    let tag = format!("v{version}");
    let message = message_template.replace("%s", &version.to_string());
    let mut add_args = vec![OsStr::new("add")];
    add_args.extend(paths.iter().map(|path| path.as_os_str()));
    git_status(Command::new("git").args(add_args), "git add")?;
    git_status(
        Command::new("git").args(["commit", "-m", message.as_str()]),
        "git commit",
    )?;

    if sign_git_tag {
        git_status(
            Command::new("git").args(["tag", "-s", tag.as_str(), "-m", tag.as_str()]),
            "git tag",
        )?;
    } else {
        git_status(
            Command::new("git").args(["tag", "--no-sign", tag.as_str()]),
            "git tag",
        )?;
    }

    Ok(())
}

fn git_output<const N: usize>(args: [&str; N]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;

    if !output.status.success() {
        return Err(command_error("git", &output));
    }

    String::from_utf8(output.stdout).map_err(|_| String::from("git returned invalid UTF-8"))
}

fn git_status(command: &mut Command, label: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("failed to run {label}: {error}"))?;

    if !output.status.success() {
        return Err(command_error(label, &output));
    }

    Ok(())
}

fn command_error(label: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };

    if detail.is_empty() {
        format!("{label} failed with status {}", output.status)
    } else {
        format!("{label} failed: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn bumps_patch_version() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

        let update =
            update_manifest_version(manifest, &VersionRequest::Bump(Bump::Patch), None).unwrap();

        assert_eq!(update.package_name, "demo");
        assert_eq!(update.old_version.to_string(), "1.2.3");
        assert_eq!(update.new_version.to_string(), "1.2.4");
        assert!(update.content.contains("version = \"1.2.4\""));
    }

    #[test]
    fn bumps_minor_version() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

        let update =
            update_manifest_version(manifest, &VersionRequest::Bump(Bump::Minor), None).unwrap();

        assert_eq!(update.new_version.to_string(), "1.3.0");
        assert!(update.content.contains("version = \"1.3.0\""));
    }

    #[test]
    fn bumps_major_version() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

        let update =
            update_manifest_version(manifest, &VersionRequest::Bump(Bump::Major), None).unwrap();

        assert_eq!(update.new_version.to_string(), "2.0.0");
        assert!(update.content.contains("version = \"2.0.0\""));
    }

    #[test]
    fn patch_clears_prerelease_and_build_metadata() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3-alpha.1+build.9\"\n";

        let update =
            update_manifest_version(manifest, &VersionRequest::Bump(Bump::Patch), None).unwrap();

        assert_eq!(update.new_version.to_string(), "1.2.3");
    }

    #[test]
    fn patch_removes_prerelease_without_incrementing() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3-alpha.1\"\n";

        let update =
            update_manifest_version(manifest, &VersionRequest::Bump(Bump::Patch), None).unwrap();

        assert_eq!(update.new_version.to_string(), "1.2.3");
    }

    #[test]
    fn sets_exact_version() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

        let update = update_manifest_version(
            manifest,
            &VersionRequest::Exact(Version::parse("2.4.6").unwrap()),
            None,
        )
        .unwrap();

        assert_eq!(update.new_version.to_string(), "2.4.6");
        assert!(update.content.contains("version = \"2.4.6\""));
    }

    #[test]
    fn bumps_preminor_with_preid() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

        let update =
            update_manifest_version(manifest, &VersionRequest::Bump(Bump::Preminor), Some("rc"))
                .unwrap();

        assert_eq!(update.new_version.to_string(), "1.3.0-rc.0");
    }

    #[test]
    fn increments_prerelease() {
        let manifest = "[package]\nname = \"demo\"\nversion = \"1.3.0-rc.0\"\n";

        let update = update_manifest_version(
            manifest,
            &VersionRequest::Bump(Bump::Prerelease),
            Some("rc"),
        )
        .unwrap();

        assert_eq!(update.new_version.to_string(), "1.3.0-rc.1");
    }

    #[test]
    fn preserves_manifest_comments() {
        let manifest = "[package]\nname = \"demo\"\n# release line\nversion = \"0.9.9\"\n";

        let update =
            update_manifest_version(manifest, &VersionRequest::Bump(Bump::Minor), None).unwrap();

        assert!(update.content.contains("# release line"));
        assert!(update.content.contains("version = \"0.10.0\""));
    }

    #[test]
    fn updates_matching_cargo_lock_package() {
        let lockfile = "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"demo\"\nversion = \"1.2.3\"\n\n[[package]]\nname = \"other\"\nversion = \"1.2.3\"\n";

        let updated = update_lockfile_package(
            lockfile,
            "demo",
            &Version::parse("1.2.3").unwrap(),
            &Version::parse("1.2.4").unwrap(),
        )
        .unwrap();

        assert!(updated.contains("name = \"demo\"\nversion = \"1.2.4\""));
        assert!(updated.contains("name = \"other\"\nversion = \"1.2.3\""));
    }

    #[test]
    fn rejects_lockfile_without_matching_package() {
        let lockfile = "[[package]]\nname = \"other\"\nversion = \"1.2.3\"\n";

        assert!(
            update_lockfile_package(
                lockfile,
                "demo",
                &Version::parse("1.2.3").unwrap(),
                &Version::parse("1.2.4").unwrap(),
            )
            .is_err()
        );
    }

    #[test]
    fn renders_help() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains("cargo-version"));
        assert!(help.contains("major"));
        assert!(help.contains("minor"));
        assert!(help.contains("patch"));
        assert!(help.contains("--no-git-tag-version"));
        assert!(help.contains("--message"));
        assert!(help.contains("--sign-git-tag"));
        assert!(help.contains("--preid"));
    }
}
