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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let saved = keys
                .iter()
                .map(|&k| (k.to_owned(), std::env::var(k).ok()))
                .collect();
            for &k in keys {
                unsafe { std::env::remove_var(k) };
            }
            EnvGuard { saved }
        }

        fn set(self, key: &str, value: &str) -> Self {
            unsafe { std::env::set_var(key, value) };
            self
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, original) in &self.saved {
                match original {
                    Some(v) => unsafe { std::env::set_var(key, v) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    const ALL_VARS: &[&str] = &[
        "CHECK_INTERVAL_SECONDS",
        "IMAGE_SOURCE",
        "IMAGE_SOURCE_FILE_PATH",
        "IMAGE_SOURCE_KUBERNETES_CONFIG_MAP_NAMESPACE",
        "IMAGE_SOURCE_KUBERNETES_CONFIG_MAP_NAME",
    ];

    #[test]
    #[serial]
    fn defaults_when_no_env_vars() {
        let _guard = EnvGuard::new(ALL_VARS);
        let config = AppConfig::from_env();
        assert_eq!(config.check_interval_seconds, 60);
        assert!(matches!(
            config.image_source,
            ImageSourceConfig::LocalFile { ref file_path } if file_path == "config.json"
        ));
    }

    #[test]
    #[serial]
    fn custom_check_interval() {
        let _guard = EnvGuard::new(ALL_VARS).set("CHECK_INTERVAL_SECONDS", "30");
        let config = AppConfig::from_env();
        assert_eq!(config.check_interval_seconds, 30);
    }

    #[test]
    #[serial]
    fn custom_file_path() {
        let _guard = EnvGuard::new(ALL_VARS).set("IMAGE_SOURCE_FILE_PATH", "custom.yml");
        let config = AppConfig::from_env();
        assert!(matches!(
            config.image_source,
            ImageSourceConfig::LocalFile { ref file_path } if file_path == "custom.yml"
        ));
    }

    #[test]
    #[serial]
    fn kubernetes_config_map_source() {
        let _guard = EnvGuard::new(ALL_VARS)
            .set("IMAGE_SOURCE", "kubernetes_config_map")
            .set("IMAGE_SOURCE_KUBERNETES_CONFIG_MAP_NAMESPACE", "production")
            .set("IMAGE_SOURCE_KUBERNETES_CONFIG_MAP_NAME", "tracker-config");
        let config = AppConfig::from_env();
        assert!(matches!(
            config.image_source,
            ImageSourceConfig::KubernetesConfigMap { ref namespace, ref name }
                if namespace == "production" && name == "tracker-config"
        ));
    }

    #[test]
    #[serial]
    fn kubernetes_config_map_defaults_namespace_and_name() {
        let _guard = EnvGuard::new(ALL_VARS).set("IMAGE_SOURCE", "kubernetes_config_map");
        let config = AppConfig::from_env();
        assert!(matches!(
            config.image_source,
            ImageSourceConfig::KubernetesConfigMap { ref namespace, ref name }
                if namespace == "default" && name == "image-config"
        ));
    }

    #[test]
    #[serial]
    fn unknown_image_source_falls_back_to_local_file() {
        let _guard = EnvGuard::new(ALL_VARS).set("IMAGE_SOURCE", "unknown_value");
        let config = AppConfig::from_env();
        assert!(matches!(
            config.image_source,
            ImageSourceConfig::LocalFile { ref file_path } if file_path == "config.json"
        ));
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