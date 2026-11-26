use std::{
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use eframe::egui::Context;
use semver::Version;
use serde::Deserialize;

use crate::jobs::Failure;

pub const RELEASE_PAGE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases/latest");

pub enum ReleaseStatus {
    Unpublished,
    Current,
    Available(String),
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

pub fn start(context: &Context) -> Receiver<Result<ReleaseStatus, Failure>> {
    let context = context.clone();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        if send.send(check()).is_ok() {
            context.request_repaint();
        }
    });
    receive
}

fn check() -> Result<ReleaseStatus, Failure> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let response = agent
        .get("https://api.github.com/repos/yrmuq/tenka/releases/latest")
        .header("User-Agent", concat!("Tenka/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call();
    let mut response = match response {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(ReleaseStatus::Unpublished),
        Err(ureq::Error::StatusCode(status)) => {
            return Err(Failure {
                label: 159,
                detail: format!("HTTP {status}"),
            });
        }
        Err(error) => {
            return Err(Failure {
                label: 160,
                detail: error.to_string(),
            });
        }
    };
    let release = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_json::<Release>()
        .map_err(|error| Failure {
            label: 161,
            detail: error.to_string(),
        })?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| Failure {
        label: 175,
        detail: error.to_string(),
    })?;
    compare_release(release, &current)
}

fn compare_release(release: Release, current: &Version) -> Result<ReleaseStatus, Failure> {
    let tag = release.tag_name;
    let latest = Version::parse(tag.strip_prefix('v').unwrap_or(&tag))
        .ok()
        .filter(|version| !release.draft && !release.prerelease && version.pre.is_empty())
        .ok_or_else(|| Failure {
            label: 161,
            detail: tag.clone(),
        })?;
    Ok(if latest.cmp_precedence(current).is_gt() {
        ReleaseStatus::Available(tag)
    } else {
        ReleaseStatus::Current
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_newer_stable_releases_are_offered() {
        let current = Version::new(0, 2, 0);
        let compare = |tag: &str, draft, prerelease| {
            compare_release(
                Release {
                    tag_name: tag.into(),
                    draft,
                    prerelease,
                },
                &current,
            )
        };
        assert!(matches!(
            compare("v0.10.0", false, false),
            Ok(ReleaseStatus::Available(tag)) if tag == "v0.10.0"
        ));
        for tag in ["0.1.0", "v0.2.0", "v0.2.0+metadata"] {
            assert!(matches!(
                compare(tag, false, false),
                Ok(ReleaseStatus::Current)
            ));
        }
        assert!(compare("v1.0.0", true, false).is_err());
        assert!(compare("v1.0.0", false, true).is_err());
        assert!(compare("v1.0.0-rc.1", false, false).is_err());
        assert!(compare("dev-draft", false, false).is_err());
    }
}
