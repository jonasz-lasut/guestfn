//! The function release a scaffold's example/functions.yaml references: the
//! newest one published, read from the repository's tag list, so a release
//! of the function reaches new projects without a release of guestfn. The
//! lookup never fails a command: without an answer in time the scaffold gets
//! the release this guestfn was built with.

use std::sync::mpsc;
use std::time::Duration;

use guestfn_runtime::location::parse_any_reference;
use semver::Version;

use crate::inspect::client_for;

/// The function package a scaffold references when nothing newer is known.
/// It holds the whole reference so the release procedure's search for
/// `function-wasm:v` finds it.
pub const FALLBACK_FUNCTION_PACKAGE: &str = "ghcr.io/jonasz-lasut/function-wasm:v1.1.0";

/// A function's tags are its versions behind a `v`.
const TAG_PREFIX: &str = "v";

/// A tag list is a few kilobytes; this only bounds what a misbehaving
/// registry can hand back.
const TAG_LIST_LIMIT: usize = 1 << 20;

/// What the whole lookup may take: the registry's challenge, its token and
/// the list, about half a second against ghcr.io.
const LOOKUP_BUDGET: Duration = Duration::from_secs(3);

/// The release to write and the line that says where it came from.
pub struct Chosen {
    pub package: String,
    pub note: Note,
}

pub enum Note {
    /// The registry answered.
    Found(String),
    /// Nothing was looked up or nothing came back.
    Fallback(String),
}

impl From<Chosen> for (String, Option<Note>) {
    fn from(chosen: Chosen) -> Self {
        (chosen.package, Some(chosen.note))
    }
}

pub fn fallback_repository() -> &'static str {
    split(FALLBACK_FUNCTION_PACKAGE).0
}

fn fallback_version() -> Version {
    parse_version(split(FALLBACK_FUNCTION_PACKAGE).1).expect("the compiled package is a release")
}

fn split(package: &str) -> (&str, &str) {
    package
        .rsplit_once(':')
        .expect("the compiled package carries a tag")
}

/// A version as a flag states it, with or without the tag's `v`.
pub fn parse_version(stated: &str) -> Result<Version, String> {
    stated
        .strip_prefix(TAG_PREFIX)
        .unwrap_or(stated)
        .parse()
        .map_err(|e| format!("{stated:?} is not a version: {e}"))
}

pub fn package(repository: &str, version: &Version) -> String {
    format!("{repository}:{TAG_PREFIX}{version}")
}

/// The package this guestfn was built with, at the repository asked for.
pub fn compiled(repository: &str, why: &str) -> Chosen {
    let package = package(repository, &fallback_version());
    Chosen {
        note: Note::Fallback(format!(
            "example/functions.yaml references {package}, the release this guestfn was built with: {why}; --function-version states another"
        )),
        package,
    }
}

/// The newest release of the compiled major the repository holds, never one
/// older than the compiled release. A malformed repository is the caller's
/// mistake and the only error.
pub fn current(repository: &str) -> Result<Chosen, String> {
    let floor = fallback_version();
    let reference = parse_any_reference(&package(repository, &floor))?;
    // The answer is waited for on a channel so the budget bounds the lookup
    // as a whole, whatever the registry client's own timeout is.
    let (answer, waiting) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = answer.send(client_for(&reference).tags(TAG_LIST_LIMIT));
    });
    let tags = match waiting.recv_timeout(LOOKUP_BUDGET) {
        Ok(Ok(tags)) => tags,
        Ok(Err(e)) => {
            return Ok(compiled(
                repository,
                &format!("cannot list the releases at {repository}: {e}"),
            ));
        }
        Err(_) => {
            return Ok(compiled(
                repository,
                &format!(
                    "{repository} did not list its releases within {} seconds",
                    LOOKUP_BUDGET.as_secs()
                ),
            ));
        }
    };
    let version = newest(&tags, TAG_PREFIX, |v| v.major == floor.major)
        .filter(|found| *found > floor)
        .unwrap_or_else(|| floor.clone());
    let package = package(repository, &version);
    let mut line = format!(
        "example/functions.yaml references {package}, the newest {}.x release at {repository}",
        floor.major
    );
    if let Some(next) = newest(&tags, TAG_PREFIX, |v| v.major > floor.major) {
        line.push_str(&format!(
            "\n{TAG_PREFIX}{next} is published: a new major, which --function-version {TAG_PREFIX}{next} writes"
        ));
    }
    Ok(Chosen {
        package,
        note: Note::Found(line),
    })
}

/// The newest release among a repository's tags: a tag counts when it is
/// the prefix and a version with no pre-release, which passes over the
/// referrers' `sha256-` tags and anything else a repository holds.
pub fn newest(tags: &[String], prefix: &str, keep: impl Fn(&Version) -> bool) -> Option<Version> {
    tags.iter()
        .filter_map(|tag| tag.strip_prefix(prefix)?.parse::<Version>().ok())
        .filter(|version| version.pre.is_empty() && keep(version))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(published: &[&str]) -> Vec<String> {
        published.iter().map(|tag| tag.to_string()).collect()
    }

    #[test]
    fn passes_over_prereleases_and_tags_that_are_no_version() {
        let published = tags(&[
            "1.0.0",
            "1.1.0",
            "1.10.0",
            "1.11.0-rc.1",
            "sha256-abc",
            "latest",
            "2.0.0",
            "v1.12.0",
        ]);
        assert_eq!(
            newest(&published, "", |v| v.major == 1),
            Some(Version::new(1, 10, 0))
        );
        assert_eq!(
            newest(&published, "", |v| v.major > 1),
            Some(Version::new(2, 0, 0))
        );
        assert_eq!(newest(&published, "", |v| v.major == 3), None);
    }

    #[test]
    fn a_function_release_is_a_v_and_a_version() {
        let published = tags(&[
            "v0.6.0",
            "v1.0.0",
            "sha256-d48b67e8187583de01506eb58439a57fd386795ccb8ee8eaa27df532d062554d",
            "v1.0.2",
            "v1.0.10",
            "v1.1.0-rc.1",
            "1.4.0",
            "v2.0.0",
        ]);
        assert_eq!(
            newest(&published, TAG_PREFIX, |v| v.major == 1),
            Some(Version::new(1, 0, 10))
        );
        assert_eq!(
            newest(&published, TAG_PREFIX, |v| v.major > 1),
            Some(Version::new(2, 0, 0))
        );
    }

    #[test]
    fn a_stated_version_may_carry_the_v() {
        assert_eq!(parse_version("v1.2.3"), Ok(Version::new(1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Ok(Version::new(1, 2, 3)));
        assert!(parse_version("latest").is_err());
    }

    #[test]
    fn the_compiled_package_is_a_release_of_a_repository() {
        assert_eq!(
            package(fallback_repository(), &fallback_version()),
            FALLBACK_FUNCTION_PACKAGE
        );
    }
}
