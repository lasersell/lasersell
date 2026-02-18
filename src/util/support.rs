const SUPPORT_URL: &str = "discord.gg/lasersell";
const SUPPORT_HINT: &str = "Need help? Report it in discord.gg/lasersell.";

pub fn with_support_hint(message: impl Into<String>) -> String {
    let mut text = message.into();
    if text.contains(SUPPORT_URL) {
        return text;
    }
    let trimmed = text.trim_end();
    if trimmed.len() != text.len() {
        text = trimmed.to_string();
    }
    if !text.is_empty() {
        let last = text.chars().last().unwrap();
        if !matches!(last, '.' | '!' | '?') {
            text.push('.');
        }
        text.push(' ');
    }
    text.push_str(SUPPORT_HINT);
    text
}
