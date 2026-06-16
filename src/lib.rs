use anyhow::Ok;
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
pub struct TrackedImage {
    /// The name of the image to track, e.g. `nginx`.
    name: String,
    /// The strategy to use when tracking this image.
    strategy: TrackingStrategy,
}

pub enum TrackedImageResult {
    Semver(semver::Version),
    Digest(String),
}

pub async fn load_tracked_images(config: &AppConfig) -> anyhow::Result<Vec<TrackedImage>> {
    match &config.image_source {
        appconfig::ImageSourceConfig::LocalFile { file_path } => {
            let file = fs::File::open(file_path).await?;
            let tracked_images: Vec<TrackedImage> = serde_yaml::from_reader(file.into_std().await)?;
            Ok(tracked_images)
        }
        appconfig::ImageSourceConfig::KubernetesConfigMap {
            namespace: _,
            name: _,
        } => Err(anyhow::anyhow!(
            "KubernetesConfigMap source not implemented"
        )),
    }
}

/// Fetches the latest version of the given image according to its tracking strategy.
pub async fn fetch_latest(
    image: TrackedImage,
) -> Result<Option<TrackedImageResult>, anyhow::Error> {
    tracing::debug!("Fetching image: {:?}", &image);
    let client = Client::new(ClientConfig::default());
    let reference: Reference = image.name.parse()?;

    match image.strategy {
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
