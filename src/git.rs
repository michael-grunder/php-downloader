use std::{
    fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};
use bzip2::{Compression, write::BzEncoder};
use chrono::{DateTime, Utc};
use regex::Regex;
use tempfile::NamedTempFile;

use crate::{
    Config,
    downloads::{
        DownloadInfo, DownloadSource, Extension, Version,
        git_source_marker_path,
    },
};

const PHP_SRC_URL: &str = "https://github.com/php/php-src.git";
const PHP_SRC_WEB_URL: &str = "https://github.com/php/php-src";
const OLDEST_GIT_MAJOR: u8 = 8;
const OLDEST_GIT_MINOR: u8 = 5;

#[derive(Clone, Debug)]
pub struct GitSource {
    path: PathBuf,
    remote: String,
}

#[derive(Debug)]
pub struct GitArchive {
    pub path: PathBuf,
    pub label: String,
    pub source_url: String,
}

#[derive(Debug)]
struct ResolvedRevision {
    archive_ref: String,
    label: String,
    source_url: String,
}

impl GitSource {
    pub fn open() -> Result<Self> {
        Self::open_at(Config::git_repository_path()?, PHP_SRC_URL)
    }

    fn open_at(path: PathBuf, remote: &str) -> Result<Self> {
        let source = Self {
            path,
            remote: remote.to_string(),
        };

        if source.path.exists() {
            source.validate()?;
            source.update()?;
        } else {
            source.clone_repository()?;
        }

        Ok(source)
    }

    fn clone_repository(&self) -> Result<()> {
        let parent = self.path.parent().ok_or_else(|| {
            anyhow!("Git repository path {} has no parent", self.path.display())
        })?;
        fs::create_dir_all(parent).with_context(|| {
            format!("Unable to create git cache directory {}", parent.display())
        })?;

        eprintln!("Cloning php-src into {}", self.path.display());
        let temp = tempfile::Builder::new()
            .prefix("php-src-clone-")
            .tempdir_in(parent)
            .with_context(|| {
                format!(
                    "Unable to create temporary directory in {}",
                    parent.display()
                )
            })?;
        let cloned_path = temp.path().join("php-src.git");
        let output = Command::new("git")
            .args(["clone", "--bare", "--filter=blob:none"])
            .arg(&self.remote)
            .arg(&cloned_path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .context("Unable to execute git clone")?;
        check_git_output("clone php-src", &output)?;

        fs::rename(&cloned_path, &self.path).with_context(|| {
            format!(
                "Unable to install cloned php-src repository at {}",
                self.path.display()
            )
        })?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        let output = self.run(["rev-parse", "--is-bare-repository"])?;
        check_git_output("validate cached php-src repository", &output)?;
        if String::from_utf8_lossy(&output.stdout).trim() != "true" {
            bail!(
                "Cached php-src path {} is not a bare git repository",
                self.path.display()
            );
        }
        Ok(())
    }

    fn update(&self) -> Result<()> {
        eprintln!("Updating cached php-src repository");
        let output =
            self.run(["fetch", "--force", "--prune", "--tags", "origin"])?;
        check_git_output("update cached php-src repository", &output)
    }

    fn run<const N: usize>(&self, args: [&str; N]) -> Result<Output> {
        Command::new("git")
            .arg("--git-dir")
            .arg(&self.path)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .with_context(|| {
                format!(
                    "Unable to execute git using repository {}",
                    self.path.display()
                )
            })
    }

    pub fn list(&self, filter: Option<Version>) -> Result<Vec<DownloadInfo>> {
        let output = self.run([
            "for-each-ref",
            "--format=%(refname:short)%09%(creatordate:unix)",
            "refs/tags",
        ])?;
        check_git_output("list php-src tags", &output)?;

        let stdout = String::from_utf8(output.stdout)
            .context("Git returned non-UTF-8 tag information")?;
        let mut tags = stdout
            .lines()
            .filter_map(|line| Self::tag_download_info(line, filter))
            .collect::<Vec<_>>();
        tags.sort_unstable_by_key(|tag| tag.version);
        Ok(tags)
    }

    fn tag_download_info(
        line: &str,
        filter: Option<Version>,
    ) -> Option<DownloadInfo> {
        let (tag, timestamp) = line.split_once('\t')?;
        let version = parse_php_tag(tag)?;
        if (version.major, version.minor)
            <= (OLDEST_GIT_MAJOR, OLDEST_GIT_MINOR)
            || !version.optional_matches(filter)
        {
            return None;
        }

        let date = timestamp
            .parse::<i64>()
            .ok()
            .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0));
        Some(DownloadInfo::new(
            version,
            &format!("{PHP_SRC_WEB_URL}/tree/{tag}"),
            0,
            date,
            Extension::BZ,
            DownloadSource::Git,
        ))
    }

    pub fn materialize(
        &self,
        revision: &str,
        destination: &Path,
        overwrite: bool,
    ) -> Result<GitArchive> {
        let resolved = self.resolve(revision)?;
        fs::create_dir_all(destination).with_context(|| {
            format!(
                "Unable to create archive directory {}",
                destination.display()
            )
        })?;
        let path = destination.join(format!("php-{}.tar.bz2", resolved.label));

        if path.exists() && !overwrite {
            if !git_source_marker_path(&path).exists() {
                bail!(
                    "Archive {} already exists but is not marked as git-sourced; use --force to replace it",
                    path.display()
                );
            }
        } else {
            self.write_archive(&resolved, &path)?;
            Self::write_source_marker(&path, &resolved.source_url)?;
        }

        Ok(GitArchive {
            path,
            label: resolved.label,
            source_url: resolved.source_url,
        })
    }

    fn resolve(&self, revision: &str) -> Result<ResolvedRevision> {
        let unprefixed = revision.strip_prefix("php-").unwrap_or(revision);
        if let Ok(version) = unprefixed.parse::<Version>() {
            if version.patch.is_none() {
                bail!("Git tags must include a patch version");
            }
            let tag = format!("php-{version}");
            self.resolve_commit(&format!("refs/tags/{tag}"))?;
            return Ok(ResolvedRevision {
                archive_ref: tag.clone(),
                label: version.to_string(),
                source_url: format!("{PHP_SRC_WEB_URL}/tree/{tag}"),
            });
        }

        let sha_pattern = Regex::new(r"(?i)^[0-9a-f]{7,40}$")?;
        if !sha_pattern.is_match(revision) {
            bail!(
                "Git revision must be a php-X.Y.Z tag, X.Y.Z tag, or 7-40 character commit SHA"
            );
        }

        let commit = if let Ok(commit) = self.resolve_commit(revision) {
            commit
        } else {
            let output = self.run(["fetch", "--force", "origin", revision])?;
            check_git_output("fetch requested php-src commit", &output)?;
            self.resolve_commit(revision)?
        };
        let version = self.version_at_commit(&commit)?;
        let short_commit = &commit[..12.min(commit.len())];
        Ok(ResolvedRevision {
            archive_ref: commit.clone(),
            label: format!("{version}-git-{short_commit}"),
            source_url: format!("{PHP_SRC_WEB_URL}/commit/{commit}"),
        })
    }

    fn resolve_commit(&self, revision: &str) -> Result<String> {
        let expression = format!("{revision}^{{commit}}");
        let output = self.run(["rev-parse", "--verify", &expression])?;
        check_git_output("resolve php-src revision", &output)?;
        Ok(String::from_utf8(output.stdout)
            .context("Git returned a non-UTF-8 commit ID")?
            .trim()
            .to_string())
    }

    fn version_at_commit(&self, commit: &str) -> Result<Version> {
        let object = format!("{commit}:main/php_version.h");
        let output = self.run(["show", &object])?;
        check_git_output("read PHP version from git commit", &output)?;
        let header = String::from_utf8(output.stdout)
            .context("PHP version header is not valid UTF-8")?;
        parse_php_version_header(&header)
    }

    fn write_archive(
        &self,
        revision: &ResolvedRevision,
        destination: &Path,
    ) -> Result<()> {
        let parent = destination.parent().ok_or_else(|| {
            anyhow!(
                "Archive destination {} has no parent directory",
                destination.display()
            )
        })?;
        let mut temp = NamedTempFile::new_in(parent).with_context(|| {
            format!(
                "Unable to create temporary archive in {}",
                parent.display()
            )
        })?;
        let mut permissions = fs::metadata(temp.path())?.permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(temp.path(), permissions)?;

        let prefix = format!("php-{}/", revision.label);
        let mut child = Command::new("git")
            .arg("--git-dir")
            .arg(&self.path)
            .args(["archive", "--format=tar", "--prefix", &prefix])
            .arg(&revision.archive_ref)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("Unable to execute git archive")?;
        let mut stdout = child
            .stdout
            .take()
            .context("Unable to read git archive output")?;

        let copy_result = (|| -> Result<()> {
            let mut encoder =
                BzEncoder::new(temp.as_file_mut(), Compression::best());
            io::copy(&mut stdout, &mut encoder)
                .context("Unable to compress git archive")?;
            encoder.finish().context("Unable to finish git archive")?;
            Ok(())
        })();
        drop(stdout);
        let status = child.wait().context("Unable to wait for git archive")?;
        copy_result?;
        if !status.success() {
            bail!("Unable to create php-src archive: git exited with {status}");
        }
        temp.as_file_mut().flush()?;
        temp.persist(destination).map_err(|error| {
            anyhow!(
                "Unable to persist git archive at {}: {}",
                destination.display(),
                error.error
            )
        })?;
        Ok(())
    }

    fn write_source_marker(archive: &Path, source_url: &str) -> Result<()> {
        let marker = git_source_marker_path(archive);
        let parent = marker.parent().ok_or_else(|| {
            anyhow!("Git source marker {} has no parent", marker.display())
        })?;
        let mut temp = NamedTempFile::new_in(parent)?;
        writeln!(temp, "{source_url}")?;
        temp.persist(&marker).map_err(|error| {
            anyhow!(
                "Unable to save git source marker {}: {}",
                marker.display(),
                error.error
            )
        })?;
        Ok(())
    }
}

fn parse_php_tag(tag: &str) -> Option<Version> {
    let version = tag.strip_prefix("php-")?.parse::<Version>().ok()?;
    version.patch.map(|_| version)
}

fn parse_php_version_header(header: &str) -> Result<Version> {
    fn component(header: &str, name: &str) -> Result<u8> {
        let pattern =
            Regex::new(&format!(r"(?m)^#define\s+{name}\s+(\d+)\s*$"))?;
        pattern
            .captures(header)
            .and_then(|captures| captures.get(1))
            .context(format!("Unable to find {name} in main/php_version.h"))?
            .as_str()
            .parse::<u8>()
            .with_context(|| format!("Invalid {name} in main/php_version.h"))
    }

    Ok(Version::from_major_minor_patch(
        component(header, "PHP_MAJOR_VERSION")?,
        component(header, "PHP_MINOR_VERSION")?,
        component(header, "PHP_RELEASE_VERSION")?,
    ))
}

fn check_git_output(operation: &str, output: &Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("Unable to {operation}: {}", stderr.trim());
}

#[cfg(test)]
mod tests {
    use bzip2::read::BzDecoder;
    use tar::Archive;

    use super::*;

    fn run_fixture_git(repository: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .env("GIT_AUTHOR_NAME", "php-downloader test")
            .env("GIT_AUTHOR_EMAIL", "php-downloader@example.invalid")
            .env("GIT_COMMITTER_NAME", "php-downloader test")
            .env("GIT_COMMITTER_EMAIL", "php-downloader@example.invalid")
            .output()
            .expect("fixture git command should execute");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("git output should be UTF-8")
    }

    fn assert_collision_handling(source: &GitSource, registry: &Path) {
        let collision = registry.join("php-8.6.0beta1.tar.bz2");
        fs::write(&collision, "existing non-git archive")
            .expect("colliding archive");
        let collision_error = source
            .materialize("8.6.0beta1", registry, false)
            .expect_err("unmarked archive must not be overwritten");
        assert!(collision_error.to_string().contains("use --force"));
        let replaced = source
            .materialize("8.6.0beta1", registry, true)
            .expect("force replaces colliding archive");
        assert!(git_source_marker_path(&replaced.path).is_file());
    }

    #[test]
    fn parses_only_php_release_tags() {
        assert_eq!(
            parse_php_tag("php-8.6.0alpha2"),
            Some(Version::new(
                8,
                6,
                Some(0),
                Some(crate::downloads::VersionModifier::Alpha(2)),
            ))
        );
        assert_eq!(parse_php_tag("PHP-8.6.0"), None);
        assert_eq!(parse_php_tag("php-8.6"), None);
        assert_eq!(parse_php_tag("php-8.6.0-dev"), None);
        assert_eq!(parse_php_tag("before-php-8.6.0"), None);
    }

    #[test]
    fn reads_version_components_from_php_header() {
        let header = r#"
#define PHP_MAJOR_VERSION 8
#define PHP_MINOR_VERSION 7
#define PHP_RELEASE_VERSION 0
#define PHP_EXTRA_VERSION "-dev"
"#;
        assert_eq!(
            parse_php_version_header(header).expect("version should parse"),
            Version::from_major_minor_patch(8, 7, 0)
        );
    }

    #[test]
    fn clones_lists_and_materializes_tags_and_commits() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let upstream = temp.path().join("upstream");
        fs::create_dir(&upstream).expect("upstream directory");
        run_fixture_git(&upstream, &["init", "--initial-branch=main"]);
        fs::create_dir(upstream.join("main")).expect("main directory");
        fs::write(
            upstream.join("main/php_version.h"),
            concat!(
                "#define PHP_MAJOR_VERSION 8\n",
                "#define PHP_MINOR_VERSION 6\n",
                "#define PHP_RELEASE_VERSION 0\n",
            ),
        )
        .expect("version header");
        fs::write(upstream.join("README.md"), "php-src fixture\n")
            .expect("fixture readme");
        run_fixture_git(&upstream, &["add", "."]);
        run_fixture_git(&upstream, &["commit", "-m", "fixture"]);
        run_fixture_git(&upstream, &["tag", "php-8.5.99"]);
        run_fixture_git(&upstream, &["tag", "php-8.6.0alpha2"]);

        let cache = temp.path().join("cache/php-src.git");
        GitSource::open_at(
            cache,
            upstream.to_str().expect("UTF-8 fixture path"),
        )
        .expect("clone fixture repository");
        fs::write(upstream.join("README.md"), "updated php-src fixture\n")
            .expect("updated fixture readme");
        run_fixture_git(&upstream, &["add", "README.md"]);
        run_fixture_git(&upstream, &["commit", "-m", "updated fixture"]);
        run_fixture_git(&upstream, &["tag", "php-8.6.0beta1"]);
        let source = GitSource::open_at(
            temp.path().join("cache/php-src.git"),
            upstream.to_str().expect("UTF-8 fixture path"),
        )
        .expect("update fixture repository");
        let tags = source.list(None).expect("list tags");
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "8.6.0alpha2");
        assert_eq!(tags[0].source, DownloadSource::Git);
        assert_eq!(tags[1].name, "8.6.0beta1");

        let registry = temp.path().join("registry");
        let tagged = source
            .materialize("8.6.0alpha2", &registry, false)
            .expect("materialize tag");
        assert_eq!(tagged.label, "8.6.0alpha2");
        assert!(git_source_marker_path(&tagged.path).is_file());
        let cached = DownloadInfo::from_file(&tagged.path)
            .expect("read cached git archive metadata");
        assert_eq!(cached.name, "8.6.0alpha2");
        assert_eq!(cached.source, DownloadSource::Git);
        let cached_json = serde_json::to_value(&cached)
            .expect("serialize cached git archive metadata");
        assert_eq!(cached_json["version"], "8.6.0alpha2");
        assert_eq!(cached_json["source"], "git");

        assert_collision_handling(&source, &registry);

        let file = fs::File::open(&tagged.path).expect("tag archive");
        let mut archive = Archive::new(BzDecoder::new(file));
        let paths = archive
            .entries()
            .expect("archive entries")
            .map(|entry| {
                entry
                    .expect("archive entry")
                    .path()
                    .expect("archive path")
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert!(
            paths
                .iter()
                .any(|path| path == Path::new("php-8.6.0alpha2/README.md"))
        );

        let extraction_root = temp.path().join("extracted");
        fs::create_dir(&extraction_root).expect("extraction directory");
        let tarball =
            crate::extract::Tarball::from_path(tagged.path, Extension::BZ)
                .expect("cached tarball");
        let extracted = tarball
            .extract(&extraction_root, None)
            .expect("extract cached git tag");
        assert!(extracted.join("README.md").is_file());
        let root = crate::extract::BuildRoot::from_path(extracted)
            .expect("recognize extracted git tag");
        assert_eq!(root.version.to_string(), "8.6.0alpha2");

        let commit = run_fixture_git(&upstream, &["rev-parse", "HEAD"]);
        let commit = commit.trim();
        let from_commit = source
            .materialize(commit, &registry, false)
            .expect("materialize commit");
        assert_eq!(from_commit.label, format!("8.6.0-git-{}", &commit[..12]));
        let cached_commit = DownloadInfo::from_file(&from_commit.path)
            .expect("read cached commit archive metadata");
        assert_eq!(cached_commit.name, from_commit.label);
        assert_eq!(cached_commit.version.to_string(), "8.6.0");
    }
}
