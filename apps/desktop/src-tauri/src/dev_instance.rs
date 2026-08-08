use std::env;

const LABEL_LIMIT: usize = 36;
const TAG_LIMIT: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DevelopmentInstance {
    label: String,
    namespace: String,
    tag: String,
}

impl DevelopmentInstance {
    pub(crate) fn from_environment() -> Option<Self> {
        Self::from_values(
            env::var("TOUCHGRASS_DEV_INSTANCE_LABEL").ok()?,
            env::var("TOUCHGRASS_DEV_NAMESPACE").ok()?,
            env::var("TOUCHGRASS_DEV_INSTANCE_TAG").ok()?,
        )
    }

    fn from_values(label: String, namespace: String, tag: String) -> Option<Self> {
        if !valid_namespace(&namespace) {
            return None;
        }
        Some(Self {
            label: bounded_value(label, LABEL_LIMIT)?,
            namespace,
            tag: bounded_value(tag, TAG_LIMIT)?,
        })
    }

    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) fn quit_label(&self) -> String {
        format!("Quit TouchGrassBar · {}", self.tag)
    }

    pub(crate) fn tag(&self) -> &str {
        &self.tag
    }

    pub(crate) fn window_title(&self, title: &str) -> String {
        format!("{title} · {}", self.label)
    }
}

fn valid_namespace(value: &str) -> bool {
    value
        .strip_prefix("app.touchgrass.bar.dev.w")
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn bounded_value(value: String, limit: usize) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = value.chars().take(limit).collect::<String>();
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::DevelopmentInstance;

    #[test]
    fn bounds_and_formats_the_native_development_identity() {
        let instance = DevelopmentInstance::from_values(
            "  Cache   refresh with an intentionally long suffix  ".to_owned(),
            "app.touchgrass.bar.dev.wexample".to_owned(),
            "CACHE-REFRESH".to_owned(),
        )
        .expect("development identity");

        assert_eq!(instance.namespace(), "app.touchgrass.bar.dev.wexample");
        assert_eq!(instance.tag(), "CACHE-RE");
        assert_eq!(instance.quit_label(), "Quit TouchGrassBar · CACHE-RE");
        assert_eq!(
            instance.window_title("TouchGrassBar Settings"),
            "TouchGrassBar Settings · Cache refresh with an intentionally"
        );
    }

    #[test]
    fn rejects_a_namespace_outside_the_development_scope() {
        assert!(
            DevelopmentInstance::from_values(
                "Development".to_owned(),
                "app.touchgrass.bar".to_owned(),
                "DEV".to_owned(),
            )
            .is_none()
        );
    }
}
