use std::time::Duration;
use tokio::time::sleep;

use version_tracker::AppConfig;

async fn monitoring_loop(config: AppConfig) {
    tracing::debug!(
        "Starting monitoring loop with interval of {} seconds",
        config.check_interval_seconds
    );
    loop {
        tracing::info!("Checking for updates...");
        let tracked_images = version_tracker::load_tracked_images(&config).await;
        let tracked_images = match tracked_images {
            Ok(images) => images,
            Err(e) => {
                tracing::error!("Error loading tracked images: {:?}", e);
                Vec::new()
            }
        };

        if tracked_images.is_empty() {
            tracing::info!(
                "No images to track, sleeping for {} seconds",
                config.check_interval_seconds
            );
            sleep(Duration::from_secs(config.check_interval_seconds)).await;
            continue;
        }

        tracked_images.into_iter().for_each(|image| {
            tokio::spawn(async move {
                match version_tracker::fetch_latest(image).await {
                    Ok(Some(version)) => match version {
                        version_tracker::TrackedImageResult::Semver(v) => {
                            tracing::info!("Found latest version: {}", v)
                        }
                        version_tracker::TrackedImageResult::Digest(d) => {
                            tracing::info!("Found latest digest: {}", d)
                        }
                    },
                    Ok(None) => tracing::info!("No version found"),
                    Err(e) => tracing::error!("Error fetching image: {:?}", e),
                }
            });
        });

        tracing::info!(
            "Finished checking for updates, sleeping for {} seconds",
            config.check_interval_seconds
        );
        sleep(Duration::from_secs(config.check_interval_seconds)).await;
    }
}

#[tokio::main]
async fn main() {
    // Load environment variables from .env file if it exists
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting version tracker...");
    let config = AppConfig::from_env();
    tracing::info!("Config: {:?}", &config);
    tokio::spawn(monitoring_loop(config)).await.unwrap();
}
