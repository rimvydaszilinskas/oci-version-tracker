use anyhow::Ok;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{Api, Client as KubeClient};
use serde::{Deserialize, Serialize};

mod appconfig;
pub use appconfig::AppConfig;
use oci_client::{Client, Reference, client::ClientConfig, secrets::RegistryAuth::Anonymous};
use tokio::fs;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TrackingStrategy {
    /// Track the latest version of a package.
    Latest,
    /// Track a specific version of a package.
    SemverPattern {
        /// The semver pattern to match against available versions.
        pattern: semver::VersionReq,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpdateStrategy {
    Filesystem {
        path: String,
        #[serde(default)]
        override_version: bool,
    },
    Git {
        repository: String,
        branch: String,
        path: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackedImage {
    /// The name of the image to track, e.g. `nginx`.
    pub name: String,
    /// The strategy to use when tracking this image.
    strategy: TrackingStrategy,
    /// The strategies to use when updating this image.
    pub update_strategies: Vec<UpdateStrategy>,
}

pub enum TrackedImageResult {
    Semver(semver::Version),
    Digest(String),
}

impl ToString for TrackedImageResult {
    fn to_string(&self) -> String {
        match self {
            TrackedImageResult::Semver(v) => v.to_string(),
            TrackedImageResult::Digest(d) => d.to_owned(),
        }
    }
}

/// Load the list of tracked images from the configured source (e.g. local file or Kubernetes ConfigMap).
pub async fn load_tracked_images(config: &AppConfig) -> anyhow::Result<Vec<TrackedImage>> {
    match &config.image_source {
        appconfig::ImageSourceConfig::LocalFile { file_path } => {
            let file = fs::File::open(file_path).await?;
            let tracked_images: Vec<TrackedImage> = serde_yaml::from_reader(file.into_std().await)?;
            Ok(tracked_images)
        }
        appconfig::ImageSourceConfig::KubernetesConfigMap { namespace, name } => {
            let client = KubeClient::try_default().await?;
            let configmaps: Api<ConfigMap> = Api::namespaced(client, namespace);
            let cm = configmaps.get(name).await?;
            let data = cm
                .data
                .ok_or_else(|| anyhow::anyhow!("ConfigMap has no data"))?;
            let yaml = data
                .get("config.yaml")
                .ok_or_else(|| anyhow::anyhow!("ConfigMap missing 'config.yaml' key"))?;
            let tracked_images: Vec<TrackedImage> = serde_yaml::from_str(yaml)?;
            Ok(tracked_images)
        }
    }
}

pub async fn apply_filesystem_update(
    image: &TrackedImage,
    result: &TrackedImageResult,
    path: &str,
    override_version: bool,
) -> anyhow::Result<()> {
    let contents = fs::read_to_string(path).await?;
    let new_version = result.to_string();

    let updated = contents
        .lines()
        .map(|line| rewrite_line(line, &image.name, &new_version, result, override_version))
        .collect::<Vec<_>>()
        .join("\n");

    // Preserve trailing newline if original had one
    let updated = if contents.ends_with('\n') {
        format!("{}\n", updated)
    } else {
        updated
    };

    fs::write(path, updated).await?;
    Ok(())
}

fn rewrite_line(
    line: &str,
    image_name: &str,
    new_version: &str,
    result: &TrackedImageResult,
    override_version: bool,
) -> String {
    let full_marker = format!("# vt:{}:full", image_name);
    let tag_marker = format!("# vt:{}:tag", image_name);

    let mode = if line.contains(&full_marker) {
        "full"
    } else if line.contains(&tag_marker) {
        "tag"
    } else {
        return line.to_owned();
    };

    // Split line into value part and comment part
    let marker = if mode == "full" { &full_marker } else { &tag_marker };
    let (value_part, comment_part) = line.split_once(marker).unwrap();
    // value_part ends with "  # " or " # " — keep that whitespace
    let value_trimmed = value_part.trim_end();
    let trailing_space = &value_part[value_trimmed.len()..];

    if mode == "full" {
        // value_trimmed looks like "  image: nginx:1.2.3" — find the image ref after the last colon
        // Split on ": " to get the yaml key and value
        if let Some((yaml_key, yaml_value)) = value_trimmed.split_once(": ") {
            let current_tag = if let Some(pos) = yaml_value.rfind(':') {
                &yaml_value[pos + 1..]
            } else {
                yaml_value
            };

            if !should_update(current_tag, result, override_version) {
                return line.to_owned();
            }

            let new_value = if let Some(pos) = yaml_value.rfind(':') {
                format!("{}{}", &yaml_value[..=pos], new_version)
            } else {
                new_version.to_owned()
            };

            format!("{}: {}{}{}{}", yaml_key, new_value, trailing_space, marker, comment_part)
        } else {
            line.to_owned()
        }
    } else {
        // mode == "tag": value_trimmed looks like "  tag: 1.2.3"
        if let Some((yaml_key, current_tag)) = value_trimmed.split_once(": ") {
            if !should_update(current_tag, result, override_version) {
                return line.to_owned();
            }
            format!("{}: {}{}{}{}", yaml_key, new_version, trailing_space, marker, comment_part)
        } else {
            line.to_owned()
        }
    }
}

fn should_update(
    current: &str,
    result: &TrackedImageResult,
    override_version: bool,
) -> bool {
    if override_version {
        return true;
    }
    match result {
        TrackedImageResult::Semver(new) => {
            match semver::Version::parse(current) {
                std::result::Result::Ok(current_ver) => new > &current_ver,
                std::result::Result::Err(_) => true,
            }
        }
        TrackedImageResult::Digest(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn semver_result(v: &str) -> TrackedImageResult {
        TrackedImageResult::Semver(semver::Version::parse(v).unwrap())
    }

    fn digest_result(d: &str) -> TrackedImageResult {
        TrackedImageResult::Digest(d.to_owned())
    }

    fn tracked_image(name: &str) -> TrackedImage {
        TrackedImage {
            name: name.to_owned(),
            strategy: TrackingStrategy::Latest,
            update_strategies: vec![],
        }
    }

    // --- should_update ---

    #[test]
    fn should_update_newer_semver() {
        assert!(should_update("1.0.0", &semver_result("1.1.0"), false));
    }

    #[test]
    fn should_update_same_semver_no_change() {
        assert!(!should_update("1.1.0", &semver_result("1.1.0"), false));
    }

    #[test]
    fn should_update_older_semver_no_change() {
        assert!(!should_update("2.0.0", &semver_result("1.1.0"), false));
    }

    #[test]
    fn should_update_unparseable_current_always_updates() {
        assert!(should_update("not-a-version", &semver_result("1.1.0"), false));
    }

    #[test]
    fn should_update_digest_always_updates() {
        assert!(should_update("sha256:abc", &digest_result("sha256:xyz"), false));
    }

    #[test]
    fn should_update_override_bypasses_semver_guard() {
        assert!(should_update("2.0.0", &semver_result("1.0.0"), true));
    }

    // --- rewrite_line ---

    #[test]
    fn rewrite_line_no_marker_unchanged() {
        let line = "        image: nginx:1.0.0";
        assert_eq!(rewrite_line(line, "nginx", "2.0.0", &semver_result("2.0.0"), false), line);
    }

    #[test]
    fn rewrite_line_wrong_image_unchanged() {
        let line = "        tag: 1.0.0 # vt:other:tag";
        assert_eq!(rewrite_line(line, "nginx", "2.0.0", &semver_result("2.0.0"), false), line);
    }

    #[test]
    fn rewrite_line_tag_mode_updates_value() {
        let line = "        tag: 1.0.0 # vt:nginx:tag";
        let result = rewrite_line(line, "nginx", "2.0.0", &semver_result("2.0.0"), false);
        assert_eq!(result, "        tag: 2.0.0 # vt:nginx:tag");
    }

    #[test]
    fn rewrite_line_full_mode_updates_tag_portion() {
        let line = "        image: nginx:1.0.0 # vt:nginx:full";
        let result = rewrite_line(line, "nginx", "2.0.0", &semver_result("2.0.0"), false);
        assert_eq!(result, "        image: nginx:2.0.0 # vt:nginx:full");
    }

    #[test]
    fn rewrite_line_tag_mode_semver_guard_blocks_older() {
        let line = "        tag: 2.0.0 # vt:nginx:tag";
        let result = rewrite_line(line, "nginx", "1.0.0", &semver_result("1.0.0"), false);
        assert_eq!(result, line);
    }

    #[test]
    fn rewrite_line_tag_mode_override_ignores_guard() {
        let line = "        tag: 2.0.0 # vt:nginx:tag";
        let result = rewrite_line(line, "nginx", "1.0.0", &semver_result("1.0.0"), true);
        assert_eq!(result, "        tag: 1.0.0 # vt:nginx:tag");
    }

    #[test]
    fn rewrite_line_full_mode_digest_always_updates() {
        let line = "        image: nginx:old-digest # vt:nginx:full";
        let result = rewrite_line(line, "nginx", "sha256:abc", &digest_result("sha256:abc"), false);
        assert_eq!(result, "        image: nginx:sha256:abc # vt:nginx:full");
    }

    #[test]
    fn rewrite_line_preserves_trailing_whitespace_before_marker() {
        let line = "        tag: 1.0.0  # vt:nginx:tag";
        let result = rewrite_line(line, "nginx", "2.0.0", &semver_result("2.0.0"), false);
        assert_eq!(result, "        tag: 2.0.0  # vt:nginx:tag");
    }

    // --- apply_filesystem_update ---

    async fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[tokio::test]
    async fn apply_filesystem_tag_line_updated() {
        let content = "        tag: 1.0.0 # vt:nginx:tag\n";
        let f = write_temp(content).await;
        let image = tracked_image("nginx");
        apply_filesystem_update(&image, &semver_result("2.0.0"), f.path().to_str().unwrap(), false)
            .await
            .unwrap();
        let updated = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert_eq!(updated, "        tag: 2.0.0 # vt:nginx:tag\n");
    }

    #[tokio::test]
    async fn apply_filesystem_full_line_updated() {
        let content = "        image: nginx:1.0.0 # vt:nginx:full\n";
        let f = write_temp(content).await;
        let image = tracked_image("nginx");
        apply_filesystem_update(&image, &semver_result("2.0.0"), f.path().to_str().unwrap(), false)
            .await
            .unwrap();
        let updated = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert_eq!(updated, "        image: nginx:2.0.0 # vt:nginx:full\n");
    }

    #[tokio::test]
    async fn apply_filesystem_only_matching_image_updated() {
        let content = "        tag: 1.0.0 # vt:nginx:tag\n        tag: 3.0.0 # vt:other:tag\n";
        let f = write_temp(content).await;
        let image = tracked_image("nginx");
        apply_filesystem_update(&image, &semver_result("2.0.0"), f.path().to_str().unwrap(), false)
            .await
            .unwrap();
        let updated = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert!(updated.contains("tag: 2.0.0 # vt:nginx:tag"));
        assert!(updated.contains("tag: 3.0.0 # vt:other:tag"));
    }

    #[tokio::test]
    async fn apply_filesystem_semver_guard_prevents_downgrade() {
        let content = "        tag: 5.0.0 # vt:nginx:tag\n";
        let f = write_temp(content).await;
        let image = tracked_image("nginx");
        apply_filesystem_update(&image, &semver_result("2.0.0"), f.path().to_str().unwrap(), false)
            .await
            .unwrap();
        let updated = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert_eq!(updated, content);
    }

    #[tokio::test]
    async fn apply_filesystem_trailing_newline_preserved() {
        let content = "        tag: 1.0.0 # vt:nginx:tag\n";
        let f = write_temp(content).await;
        let image = tracked_image("nginx");
        apply_filesystem_update(&image, &semver_result("2.0.0"), f.path().to_str().unwrap(), false)
            .await
            .unwrap();
        let updated = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert!(updated.ends_with('\n'));
    }

    #[tokio::test]
    async fn apply_filesystem_no_trailing_newline_preserved() {
        let content = "        tag: 1.0.0 # vt:nginx:tag";
        let f = write_temp(content).await;
        let image = tracked_image("nginx");
        apply_filesystem_update(&image, &semver_result("2.0.0"), f.path().to_str().unwrap(), false)
            .await
            .unwrap();
        let updated = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert!(!updated.ends_with('\n'));
    }

    #[tokio::test]
    async fn apply_filesystem_missing_file_returns_err() {
        let image = tracked_image("nginx");
        let result = apply_filesystem_update(
            &image,
            &semver_result("2.0.0"),
            "/nonexistent/path/file.yml",
            false,
        )
        .await;
        assert!(result.is_err());
    }

    // --- load_tracked_images (LocalFile) ---

    #[tokio::test]
    async fn load_tracked_images_valid_yaml() {
        let yaml = r#"
- name: nginx
  strategy:
    type: latest
  update_strategies:
    - type: filesystem
      path: ./target.yml
"#;
        let f = write_temp(yaml).await;
        let config = AppConfig {
            check_interval_seconds: 60,
            image_source: appconfig::ImageSourceConfig::LocalFile {
                file_path: f.path().to_str().unwrap().to_owned(),
            },
        };
        let images = load_tracked_images(&config).await.unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].name, "nginx");
    }

    #[tokio::test]
    async fn load_tracked_images_file_not_found() {
        let config = AppConfig {
            check_interval_seconds: 60,
            image_source: appconfig::ImageSourceConfig::LocalFile {
                file_path: "/nonexistent/path.yml".to_owned(),
            },
        };
        assert!(load_tracked_images(&config).await.is_err());
    }

    #[tokio::test]
    async fn load_tracked_images_invalid_yaml() {
        let f = write_temp("not: valid: yaml: [unclosed").await;
        let config = AppConfig {
            check_interval_seconds: 60,
            image_source: appconfig::ImageSourceConfig::LocalFile {
                file_path: f.path().to_str().unwrap().to_owned(),
            },
        };
        assert!(load_tracked_images(&config).await.is_err());
    }
}

/// Fetches the latest version of the given image according to its tracking strategy.
pub async fn fetch_latest(
    image: &TrackedImage,
) -> Result<Option<TrackedImageResult>, anyhow::Error> {
    tracing::debug!("Fetching image: {:?}", &image);
    let client = Client::new(ClientConfig::default());
    let reference: Reference = image.name.parse()?;

    match &image.strategy {
        TrackingStrategy::Latest => {
            tracing::debug!("Tracking strategy: Latest");
            // Rebuild the reference with the "latest" tag to fetch the manifest digest
            let reference = Reference::with_tag(
                reference.registry().to_owned(),
                reference.repository().to_owned(),
                "latest".to_owned(),
            );

            let digest = client.fetch_manifest_digest(&reference, &Anonymous).await?;

            tracing::debug!("Latest tag: latest {}: {:?}", reference, digest);

            Ok(Some(TrackedImageResult::Digest(digest)))
        }
        TrackingStrategy::SemverPattern { pattern } => {
            tracing::debug!("Tracking strategy: SemverPattern with pattern {}", pattern);
            let response = client.list_tags(&reference, &Anonymous, None, None).await?;
            let mut valid_tags = response
                .tags
                .into_iter()
                .map(|tag| semver::Version::parse(&tag))
                .filter(|tag| tag.is_ok())
                .map(|tag| tag.unwrap())
                .filter(|tag| pattern.matches(tag))
                .collect::<Vec<_>>();
            valid_tags.sort();

            let latest_tag = valid_tags.last();

            match latest_tag {
                None => Ok(None),
                Some(tag) => {
                    tracing::debug!("Latest tag matching pattern {}: {}", pattern, tag);
                    Ok(Some(TrackedImageResult::Semver(tag.to_owned())))
                }
            }
        }
    }
}
