//! Which release of pigeon is the newest: the highest `vMAJOR.MINOR.PATCH`
//! tag of the repository, which `git ls-remote` lists.

use std::process::Command;

use anyhow::{Context, Result, bail};

/// A release: the tag that names it and the version that tag spells.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    pub tag: String,
    version: (u64, u64, u64),
}

impl Release {
    /// The release `tag` names, if it is `v` and three numbers between dots.
    fn named(tag: &str) -> Option<Self> {
        let mut numbers = tag.strip_prefix('v')?.split('.');
        let mut number = || {
            let text = numbers.next()?;
            let plain = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
            plain.then(|| text.parse().ok()).flatten()
        };
        let version = (number()?, number()?, number()?);
        numbers.next().is_none().then(|| Self {
            tag: tag.to_owned(),
            version,
        })
    }
}

/// The newest release among the tags `ls_remote` lists, one
/// `<hash>\trefs/tags/<tag>` per line, if any tag names a release.
#[must_use]
pub fn newest(ls_remote: &str) -> Option<Release> {
    ls_remote
        .lines()
        .filter_map(|line| line.split_once("\trefs/tags/"))
        .filter_map(|(_, tag)| Release::named(tag))
        .max_by_key(|release| release.version)
}

/// The newest release of the repository at `url`.
///
/// # Errors
///
/// Fails if git cannot list the tags, or the repository has no release.
pub fn newest_of(url: &str, git: &mut Command) -> Result<Release> {
    let listing = git
        .args(["ls-remote", "--tags", "--refs", url])
        .output()
        .context("running git: install it from https://git-scm.com")?;
    if !listing.status.success() {
        bail!(
            "git could not list the releases: check the connection to {url} and run pigeon update again; the daemon keeps running the program it has"
        );
    }
    newest(&String::from_utf8_lossy(&listing.stdout)).with_context(|| {
        format!("{url} has no release yet: build a checkout with `pigeon update --path <folder>`")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "\
1111\trefs/tags/v0.8.0
2222\trefs/tags/v0.10.1
3333\trefs/tags/v0.9.0
4444\trefs/tags/v1.0.0-rc1
5555\trefs/tags/nightly
6666\trefs/tags/v0.10
7777\trefs/tags/v0.10.1.2
8888\trefs/tags/v+1.0.0
";

    #[test]
    fn the_newest_release_is_the_highest_version_not_the_last_tag() {
        let newest = newest(LISTING).unwrap();
        assert_eq!(newest.tag, "v0.10.1");
    }

    #[test]
    fn tags_that_are_not_releases_name_none() {
        assert_eq!(newest(""), None);
        assert_eq!(
            newest("1111\trefs/tags/nightly\n2222\trefs/tags/v1.0.0-rc1\n"),
            None
        );
    }
}
