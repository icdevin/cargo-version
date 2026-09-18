// Exercise release preflight through the CLI to catch changes made before a failure.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_REPO: AtomicUsize = AtomicUsize::new(0);

// Give each test its own Git repository without changing the process directory.
struct TestRepo {
    path: PathBuf,
}

impl TestRepo {
    // Use a dependency-free package so lockfile updates work without network access.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cargo-version-test-{}-{}",
            std::process::id(),
            NEXT_REPO.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let repo = Self { path };
        fs::create_dir(repo.path.join("src")).unwrap();
        fs::write(repo.path.join("src/lib.rs"), "// Release test fixture.\n").unwrap();
        fs::write(
            repo.path.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.7\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(
            repo.path.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.7\"\n",
        )
        .unwrap();
        repo.git(&["init"]);
        repo.git(&["config", "user.name", "Release Test"]);
        repo.git(&["config", "user.email", "release@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "tag.gpgsign", "false"]);
        repo.git(&["config", "core.hooksPath", ".git/no-hooks"]);
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "Initial package"]);
        repo
    }

    // Keep fixture setup errors distinct from CLI failures under test.
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    // Run the real executable so failures include the public exit code and stderr.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cargo-version"))
            .args(args)
            .env("CARGO_NET_OFFLINE", "true")
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
}

impl Drop for TestRepo {
    // Remove only this test's temporary repository, including after assertion failures.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// Existing lightweight and annotated tags must stop all release paths before mutation.
#[test]
fn existing_target_tag_leaves_files_commit_and_tag_unchanged() {
    for annotated in [false, true] {
        let repo = TestRepo::new();
        if annotated {
            repo.git(&["tag", "-a", "v0.1.8", "-m", "Existing release"]);
        } else {
            repo.git(&["tag", "v0.1.8"]);
        }
        let head = repo.git(&["rev-parse", "HEAD"]);
        let tag = repo.git(&["rev-parse", "refs/tags/v0.1.8"]);
        let manifest = fs::read(repo.path.join("Cargo.toml")).unwrap();
        let lockfile = fs::read(repo.path.join("Cargo.lock")).unwrap();

        for args in [
            vec!["patch"],
            vec!["0.1.8"],
            vec!["patch", "--sign-git-tag"],
        ] {
            let output = repo.run(&args);
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(error.contains("git tag 'v0.1.8' already exists"), "{error}");
            assert!(output.stdout.is_empty());
            assert_eq!(fs::read(repo.path.join("Cargo.toml")).unwrap(), manifest);
            assert_eq!(fs::read(repo.path.join("Cargo.lock")).unwrap(), lockfile);
            assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
            assert_eq!(repo.git(&["rev-parse", "refs/tags/v0.1.8"]), tag);
            assert!(repo.git(&["status", "--porcelain"]).is_empty());
        }
    }
}

// An available tag must still permit releases, even with similar refs in the repository.
#[test]
fn available_target_tag_commits_and_tags_updated_files() {
    let repo = TestRepo::new();
    repo.git(&["branch", "v0.1.8"]);
    repo.git(&["tag", "archive/v0.1.8"]);
    repo.git(&["tag", "v0.1.80"]);
    let head = repo.git(&["rev-parse", "HEAD"]);

    let output = repo.run(&["patch"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "v0.1.8\n");
    assert_ne!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(
        repo.git(&["rev-parse", "refs/tags/v0.1.8"]),
        repo.git(&["rev-parse", "HEAD"])
    );
    for file in ["Cargo.toml", "Cargo.lock"] {
        assert!(
            fs::read_to_string(repo.path.join(file))
                .unwrap()
                .contains("version = \"0.1.8\"")
        );
    }
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
}

// File-only updates must remain usable to reconcile a manifest with an existing tag.
#[test]
fn no_git_tag_version_allows_existing_target_tag() {
    let repo = TestRepo::new();
    repo.git(&["tag", "v0.1.8"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let tag = repo.git(&["rev-parse", "refs/tags/v0.1.8"]);

    let output = repo.run(&["patch", "--no-git-tag-version"]);
    assert!(output.status.success(), "{output:?}");
    for file in ["Cargo.toml", "Cargo.lock"] {
        assert!(
            fs::read_to_string(repo.path.join(file))
                .unwrap()
                .contains("version = \"0.1.8\"")
        );
    }
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(repo.git(&["rev-parse", "refs/tags/v0.1.8"]), tag);
}
