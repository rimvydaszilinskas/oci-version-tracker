#[derive(Debug)]
pub enum ImageSourceConfig {
    LocalFile {file_path: String},
    KubernetesConfigMap {namespace: String, name: String},
}

pub struct AppConfig {
    pub check_interval_seconds: u64,
    pub image_source: ImageSourceConfig,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let check_interval_seconds = std::env::var("CHECK_INTERVAL_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        let image_source = std::env::var("IMAGE_SOURCE").unwrap_or_else(|_| "local_file".to_string());
        Self {
            check_interval_seconds,
            image_source: match image_source.as_str() {
                "local_file" => ImageSourceConfig::LocalFile {
                    file_path: std::env::var("IMAGE_SOURCE_FILE_PATH").unwrap_or_else(|_| "config.json".to_string()),
                },
                "kubernetes_config_map" => ImageSourceConfig::KubernetesConfigMap {
                    namespace: std::env::var("IMAGE_SOURCE_KUBERNETES_CONFIG_MAP_NAMESPACE").unwrap_or_else(|_| "default".to_string()),
                    name: std::env::var("IMAGE_SOURCE_KUBERNETES_CONFIG_MAP_NAME").unwrap_or_else(|_| "image-config".to_string()),
                },
                _ => ImageSourceConfig::LocalFile {
                    file_path: "config.json".to_string(),
                },
            },
        }
    }
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfig")
            .field("check_interval_seconds", &self.check_interval_seconds)
            .field("image_source", &self.image_source)
            .finish()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            check_interval_seconds: 60,
            image_source: ImageSourceConfig::LocalFile {
                file_path: "config.json".to_string()
            },
        }
    }
}