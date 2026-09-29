//! Human-readable plan label from the credentials' subscription/tier fields.

/// e.g. `default_claude_max_20x` + `max` -> `"Max 20x"`.
pub fn plan_label(rate_limit_tier: &str, subscription_type: &str) -> String {
    let tier = rate_limit_tier.to_ascii_lowercase();
    if tier.ends_with("max_20x") {
        return "Max 20x".to_string();
    }
    if tier.ends_with("max_5x") {
        return "Max 5x".to_string();
    }
    match subscription_type.to_ascii_lowercase().as_str() {
        "pro" => "Pro".to_string(),
        "max" => "Max".to_string(),
        "team" => "Team".to_string(),
        "enterprise" => "Enterprise".to_string(),
        other if !other.is_empty() => title_case(other),
        _ => "Unknown".to_string(),
    }
}

/// Turn a `snake_case` key like `seven_day_oauth_apps` into `"Seven Day Oauth Apps"`.
pub(crate) fn humanize(key: &str) -> String {
    title_case(key)
}

fn title_case(s: &str) -> String {
    s.split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_20x_tier() {
        assert_eq!(plan_label("default_claude_max_20x", "max"), "Max 20x");
    }

    #[test]
    fn max_5x_tier() {
        assert_eq!(plan_label("default_claude_max_5x", "max"), "Max 5x");
    }

    #[test]
    fn falls_back_to_subscription_type() {
        assert_eq!(plan_label("default_claude", "pro"), "Pro");
        assert_eq!(plan_label("", "max"), "Max");
        assert_eq!(plan_label("", "team"), "Team");
        assert_eq!(plan_label("", "enterprise"), "Enterprise");
    }

    #[test]
    fn unknown_subscription_is_title_cased() {
        assert_eq!(plan_label("", "custom_plan"), "Custom Plan");
    }

    #[test]
    fn everything_empty_is_unknown() {
        assert_eq!(plan_label("", ""), "Unknown");
    }
}
