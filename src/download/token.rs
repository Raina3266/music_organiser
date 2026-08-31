use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn read_token_file(path: &Path) -> Result<String, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read token file {}: {error}", path.display()))?;
    clean_token(&contents)
}

pub(crate) fn clean_token(raw: &str) -> Result<String, String> {
    let mut value = raw.trim();
    if value.is_empty() {
        return Err("the token is empty".into());
    }

    let lower = value.to_ascii_lowercase();
    if lower.contains("const token") || lower.starts_with("token=") || lower.starts_with("token =")
    {
        value = extract_assignment_value(value)
            .ok_or_else(|| "could not find the quoted value in the token assignment".to_owned())?;
    }

    if value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
    {
        value = value[7..].trim();
    }

    value = value.trim_end_matches(';').trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let matching_quotes =
            matches!(bytes[0], b'\'' | b'"' | b'`') && bytes[0] == bytes[value.len() - 1];
        if matching_quotes {
            value = &value[1..value.len() - 1];
        }
    }

    if value.chars().any(char::is_whitespace) {
        return Err("the token contains whitespace; paste only the access-token value".into());
    }
    if value.len() < 20 {
        return Err("the token looks too short; paste the complete access token".into());
    }
    if value.len() > 16_384 {
        return Err("the token is unexpectedly large".into());
    }

    Ok(value.to_owned())
}

fn extract_assignment_value(raw: &str) -> Option<&str> {
    let equals = raw.find('=')?;
    let tail = raw[equals + 1..].trim();
    let quote_index = tail.find(['\'', '"', '`'])?;
    let quote = tail.as_bytes()[quote_index];
    let after_quote = &tail[quote_index + 1..];
    let end = after_quote
        .as_bytes()
        .iter()
        .position(|byte| *byte == quote)?;
    Some(&after_quote[..end])
}

pub(super) fn prompt_for_token(reason: &str) -> Result<Option<String>, String> {
    eprintln!();
    if !reason.is_empty() {
        eprintln!("{reason}");
    }
    eprintln!(
        "Paste a replacement Spotify access token. Do not enter your password or Client Secret."
    );
    read_token("Paste the token, or press Enter to cancel: ")
}

/// Ask one question and clean whatever was typed.
///
/// End of input counts as an empty answer, so a closed stdin declines rather
/// than failing the run.
fn read_token(question: &str) -> Result<Option<String>, String> {
    eprint!("{question}");
    io::stderr()
        .flush()
        .map_err(|error| format!("cannot flush the prompt: {error}"))?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| format!("cannot read the token: {error}"))?;
    if input.trim().is_empty() {
        return Ok(None);
    }
    clean_token(&input).map(Some)
}

#[cfg(test)]
mod tests {
    use super::clean_token;

    const TOKEN: &str = "test-token-12345678901234567890";

    #[test]
    fn accepts_plain_bearer_and_assignment_forms() {
        assert_eq!(clean_token(TOKEN).unwrap(), TOKEN);
        assert_eq!(clean_token(&format!("Bearer {TOKEN}")).unwrap(), TOKEN);
        assert_eq!(
            clean_token(&format!("const token = '{TOKEN}';")).unwrap(),
            TOKEN
        );
        assert_eq!(clean_token(&format!("\"{TOKEN}\"")).unwrap(), TOKEN);
    }

    #[test]
    fn rejects_short_or_multiword_values() {
        assert!(clean_token("short").is_err());
        assert!(clean_token(&format!("{TOKEN} extra")).is_err());
    }
}
