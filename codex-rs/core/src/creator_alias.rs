//! 造物主 Creator: map catalog-only model slugs (such as `b31-si-1.3`) to the model the
//! provider actually serves. The map comes from the `CREATOR_MODEL_ALIASES` environment
//! variable (a JSON object), which the `creator` binary sets at startup. Without it this
//! is the identity function, so upstream Codex behavior is unchanged.

use std::collections::HashMap;
use std::sync::OnceLock;

fn aliases() -> &'static HashMap<String, String> {
    static ALIASES: OnceLock<HashMap<String, String>> = OnceLock::new();
    ALIASES.get_or_init(|| {
        std::env::var("CREATOR_MODEL_ALIASES")
            .ok()
            .and_then(|raw| serde_json::from_str::<HashMap<String, String>>(&raw).ok())
            .unwrap_or_default()
    })
}

/// The model slug to send on the wire for a catalog slug.
pub(crate) fn api_model_slug(slug: &str) -> String {
    aliases()
        .get(slug)
        .filter(|target| !target.is_empty())
        .cloned()
        .unwrap_or_else(|| slug.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_slug_is_unchanged() {
        assert_eq!(super::api_model_slug("gpt-5.5"), "gpt-5.5");
    }
}
