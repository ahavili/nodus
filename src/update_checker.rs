use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::report::Reporter;

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
const REPO_SLUG: &str = "WendellXY/nodus";
const STATE_FILE: &str = "update-check.json";

#[derive(Debug, Clone, PartialEq, Eq)]
struct LatestRelease {
    tag: String,
    version: Version,
}

#[derive(Debug, Clone)]
struct CheckOptions {
    now_unix_secs: u64,
    current_version: Version,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct UpdateCheckState {
    last_attempted_at_unix_secs: Option<u64>,
    latest_known_tag: Option<String>,
    latest_known_version: Option<String>,
    last_notified_tag: Option<String>,
}

pub fn maybe_notify(store_root: &Path, reporter: &Reporter) {
    let options = match CheckOptions::for_current_binary() {
        Ok(options) => options,
        Err(_) => return,
    };

    let _ = maybe_notify_with(store_root, reporter, &options, fetch_latest_release);
}

impl CheckOptions {
    fn for_current_binary() -> Result<Self> {
        Ok(Self {
            now_unix_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system clock is before the Unix epoch")?
                .as_secs(),
            current_version: Version::parse(env!("CARGO_PKG_VERSION"))
                .context("failed to parse the current package version")?,
        })
    }
}

impl UpdateCheckState {
    fn latest_known_release(&self) -> Option<LatestRelease> {
        let tag = self.latest_known_tag.clone()?;
        let version = self.latest_known_version.as_deref()?;
        Some(LatestRelease {
            tag,
            version: Version::parse(version).ok()?,
        })
    }
}

fn maybe_notify_with<F>(
    store_root: &Path,
    reporter: &Reporter,
    options: &CheckOptions,
    fetch_latest: F,
) -> Result<()>
where
    F: FnOnce() -> Result<Option<LatestRelease>>,
{
    let state_path = state_path(store_root);
    let mut state = load_state(&state_path)?;
    let mut latest_known = state.latest_known_release();

    if should_attempt_remote_check(state.last_attempted_at_unix_secs, options.now_unix_secs) {
        state.last_attempted_at_unix_secs = Some(options.now_unix_secs);

        match fetch_latest() {
            Ok(Some(release)) => {
                state.latest_known_tag = Some(release.tag.clone());
                state.latest_known_version = Some(release.version.to_string());
                latest_known = Some(release);
            }
            Ok(None) => {}
            Err(_) => {}
        }

        persist_state(&state_path, &state)?;
    }

    let Some(latest_release) = latest_known else {
        return Ok(());
    };

    if latest_release.version <= options.current_version {
        return Ok(());
    }

    if state.last_notified_tag.as_deref() == Some(latest_release.tag.as_str()) {
        return Ok(());
    }

    reporter.warning(format!(
        "nodus {} is available (current {}); see {}",
        latest_release.version,
        options.current_version,
        install_url()
    ))?;
    state.last_notified_tag = Some(latest_release.tag);
    persist_state(&state_path, &state)
}

fn should_attempt_remote_check(
    last_attempted_at_unix_secs: Option<u64>,
    now_unix_secs: u64,
) -> bool {
    match last_attempted_at_unix_secs {
        None => true,
        Some(last_attempted) => now_unix_secs.saturating_sub(last_attempted) >= CHECK_INTERVAL_SECS,
    }
}

fn fetch_latest_release() -> Result<Option<LatestRelease>> {
    let headers = match curl_head_request(&releases_latest_url()) {
        Ok(headers) => headers,
        Err(error) if is_missing_command_error(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(location) = last_location_header(&headers) else {
        return Ok(None);
    };

    Ok(parse_latest_release_from_location(&location))
}

fn curl_head_request(url: &str) -> Result<String> {
    let output = Command::new("curl")
        .args(["-fsSLI", url])
        .output()
        .with_context(|| format!("failed to run curl for {url}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn is_missing_command_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
}

fn last_location_header(headers: &str) -> Option<String> {
    headers.lines().rev().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("location") {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn parse_latest_release_from_location(location: &str) -> Option<LatestRelease> {
    let tag = location
        .rsplit('/')
        .next()?
        .split('?')
        .next()?
        .trim()
        .to_string();
    let version = parse_release_version(&tag)?;

    Some(LatestRelease { tag, version })
}

fn parse_release_version(tag: &str) -> Option<Version> {
    let normalized = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(normalized).ok()
}

fn state_path(store_root: &Path) -> PathBuf {
    store_root.join(STATE_FILE)
}

fn releases_latest_url() -> String {
    format!("https://github.com/{REPO_SLUG}/releases/latest")
}

fn install_url() -> String {
    format!("https://github.com/{REPO_SLUG}#install")
}

fn load_state(path: &Path) -> Result<UpdateCheckState> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents).unwrap_or_default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(UpdateCheckState::default())
        }
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn persist_state(path: &Path, state: &UpdateCheckState) -> Result<()> {
    let contents =
        serde_json::to_vec_pretty(state).context("failed to serialize update check state")?;
    crate::store::write_atomic(path, &contents)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::report::{ColorMode, Reporter};

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn options(now_unix_secs: u64, current_version: &str) -> CheckOptions {
        CheckOptions {
            now_unix_secs,
            current_version: Version::parse(current_version).unwrap(),
        }
    }

    fn reporter_with_buffer() -> (Reporter, SharedBuffer) {
        let buffer = SharedBuffer::default();
        let reporter = Reporter::sink(ColorMode::Never, buffer.clone());
        (reporter, buffer)
    }

    fn read_state(path: &Path) -> UpdateCheckState {
        let contents = fs::read_to_string(path).unwrap();
        serde_json::from_str(&contents).unwrap()
    }

    #[test]
    fn parses_release_tags_with_or_without_a_v_prefix() {
        assert_eq!(
            parse_release_version("v0.3.4").unwrap(),
            Version::parse("0.3.4").unwrap()
        );
        assert_eq!(
            parse_release_version("0.3.4").unwrap(),
            Version::parse("0.3.4").unwrap()
        );
        assert!(parse_release_version("release-0.3.4").is_none());
    }

    #[test]
    fn round_trips_update_check_state() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = state_path(temp.path());
        let state = UpdateCheckState {
            last_attempted_at_unix_secs: Some(42),
            latest_known_tag: Some("v0.3.4".into()),
            latest_known_version: Some("0.3.4".into()),
            last_notified_tag: Some("v0.3.4".into()),
        };

        persist_state(&path, &state).unwrap();

        assert_eq!(load_state(&path).unwrap(), state);
    }

    #[test]
    fn notifies_once_for_a_newer_release_and_persists_state() {
        let temp = tempfile::TempDir::new().unwrap();
        let state_file = state_path(temp.path());
        let (reporter, buffer) = reporter_with_buffer();

        maybe_notify_with(temp.path(), &reporter, &options(86_400, "0.3.3"), || {
            Ok(Some(LatestRelease {
                tag: "v0.3.4".into(),
                version: Version::parse("0.3.4").unwrap(),
            }))
        })
        .unwrap();

        assert_eq!(
            buffer.contents(),
            format!(
                "warning: nodus 0.3.4 is available (current 0.3.3); see {}\n",
                install_url()
            )
        );

        let state = read_state(&state_file);
        assert_eq!(state.last_attempted_at_unix_secs, Some(86_400));
        assert_eq!(state.latest_known_tag.as_deref(), Some("v0.3.4"));
        assert_eq!(state.last_notified_tag.as_deref(), Some("v0.3.4"));
    }

    #[test]
    fn skips_remote_probe_when_the_last_attempt_is_recent() {
        let temp = tempfile::TempDir::new().unwrap();
        persist_state(
            &state_path(temp.path()),
            &UpdateCheckState {
                last_attempted_at_unix_secs: Some(100),
                latest_known_tag: Some("v0.3.4".into()),
                latest_known_version: Some("0.3.4".into()),
                last_notified_tag: None,
            },
        )
        .unwrap();
        let attempted = Cell::new(false);
        let (reporter, buffer) = reporter_with_buffer();

        maybe_notify_with(
            temp.path(),
            &reporter,
            &options(100 + CHECK_INTERVAL_SECS - 1, "0.3.3"),
            || {
                attempted.set(true);
                Ok(Some(LatestRelease {
                    tag: "v9.9.9".into(),
                    version: Version::parse("9.9.9").unwrap(),
                }))
            },
        )
        .unwrap();

        assert!(!attempted.get());
        assert_eq!(
            buffer.contents(),
            format!(
                "warning: nodus 0.3.4 is available (current 0.3.3); see {}\n",
                install_url()
            )
        );
    }

    #[test]
    fn does_not_repeat_a_notice_for_the_same_release_tag() {
        let temp = tempfile::TempDir::new().unwrap();
        persist_state(
            &state_path(temp.path()),
            &UpdateCheckState {
                last_attempted_at_unix_secs: Some(0),
                latest_known_tag: Some("v0.3.4".into()),
                latest_known_version: Some("0.3.4".into()),
                last_notified_tag: Some("v0.3.4".into()),
            },
        )
        .unwrap();
        let (reporter, buffer) = reporter_with_buffer();

        maybe_notify_with(
            temp.path(),
            &reporter,
            &options(CHECK_INTERVAL_SECS - 1, "0.3.3"),
            || panic!("throttled checks should not probe remotely"),
        )
        .unwrap();

        assert!(buffer.contents().is_empty());
    }

    #[test]
    fn notifies_again_when_a_newer_release_than_the_last_notice_appears() {
        let temp = tempfile::TempDir::new().unwrap();
        persist_state(
            &state_path(temp.path()),
            &UpdateCheckState {
                last_attempted_at_unix_secs: Some(0),
                latest_known_tag: Some("v0.3.4".into()),
                latest_known_version: Some("0.3.4".into()),
                last_notified_tag: Some("v0.3.4".into()),
            },
        )
        .unwrap();
        let (reporter, buffer) = reporter_with_buffer();

        maybe_notify_with(
            temp.path(),
            &reporter,
            &options(CHECK_INTERVAL_SECS, "0.3.3"),
            || {
                Ok(Some(LatestRelease {
                    tag: "v0.3.5".into(),
                    version: Version::parse("0.3.5").unwrap(),
                }))
            },
        )
        .unwrap();

        assert_eq!(
            buffer.contents(),
            format!(
                "warning: nodus 0.3.5 is available (current 0.3.3); see {}\n",
                install_url()
            )
        );
        assert_eq!(
            read_state(&state_path(temp.path()))
                .last_notified_tag
                .as_deref(),
            Some("v0.3.5")
        );
    }

    #[test]
    fn does_not_notify_when_current_version_is_up_to_date() {
        let temp = tempfile::TempDir::new().unwrap();
        let (reporter, buffer) = reporter_with_buffer();

        maybe_notify_with(temp.path(), &reporter, &options(0, "0.3.4"), || {
            Ok(Some(LatestRelease {
                tag: "v0.3.4".into(),
                version: Version::parse("0.3.4").unwrap(),
            }))
        })
        .unwrap();

        assert!(buffer.contents().is_empty());
    }

    #[test]
    fn does_not_notify_when_the_probe_returns_no_release() {
        let temp = tempfile::TempDir::new().unwrap();
        let (reporter, buffer) = reporter_with_buffer();

        maybe_notify_with(temp.path(), &reporter, &options(0, "0.3.3"), || Ok(None)).unwrap();

        assert!(buffer.contents().is_empty());
    }

    #[test]
    fn updates_last_attempt_time_even_when_the_probe_fails() {
        let temp = tempfile::TempDir::new().unwrap();
        let state_file = state_path(temp.path());
        let (reporter, buffer) = reporter_with_buffer();

        maybe_notify_with(temp.path(), &reporter, &options(123, "0.3.3"), || {
            anyhow::bail!("network unavailable")
        })
        .unwrap();

        assert!(buffer.contents().is_empty());
        assert_eq!(
            read_state(&state_file).last_attempted_at_unix_secs,
            Some(123)
        );
    }

    #[test]
    fn extracts_the_latest_release_from_redirect_headers() {
        let headers = "\
HTTP/2 302 \r\n\
location: https://github.com/WendellXY/nodus/releases/tag/v0.3.4\r\n\
\r\n\
HTTP/2 200 \r\n\
\r\n";

        assert_eq!(
            last_location_header(headers).as_deref(),
            Some("https://github.com/WendellXY/nodus/releases/tag/v0.3.4")
        );
        let release = parse_latest_release_from_location(
            "https://github.com/WendellXY/nodus/releases/tag/v0.3.4?foo=bar",
        )
        .unwrap();
        assert_eq!(release.tag, "v0.3.4");
        assert_eq!(release.version, Version::parse("0.3.4").unwrap());
    }

    #[test]
    fn release_urls_are_derived_from_the_repo_slug() {
        assert_eq!(
            releases_latest_url(),
            format!("https://github.com/{REPO_SLUG}/releases/latest")
        );
        assert_eq!(
            install_url(),
            format!("https://github.com/{REPO_SLUG}#install")
        );
    }
}
