//! Secret redaction: what low hides from the model and the session log.

/// Secret prefixes: the prefix, what to call it, the shortest body that counts, and the body's non-alphanumeric chars.
const SECRETS: &[(&str, &str, usize, &str)] = &[
    ("AKIA", "aws key", 16, ""),
    ("ghp_", "github token", 36, ""),
    ("gho_", "github token", 36, ""),
    ("ghs_", "github token", 36, ""),
    ("ghu_", "github token", 36, ""),
    ("ghr_", "github token", 36, ""),
    ("github_pat_", "github token", 22, "_"),
    ("sk-", "api key", 20, "_-"),
    ("oc_", "opencode key", 20, "_"),
    ("cio", "crates.io token", 30, ""),
    ("xoxb-", "slack token", 10, "-"),
    ("xoxp-", "slack token", 10, "-"),
    ("xoxa-", "slack token", 10, "-"),
    ("xoxs-", "slack token", 10, "-"),
    ("xoxe-", "slack token", 10, "-"),
    ("xapp-", "slack token", 10, "-"),
];
/// The bytes a secret can start with: the table's prefixes, and '-' for a PEM block.
const STARTS: [bool; 256] = {
    let mut starts = [false; 256];
    starts[b'-' as usize] = true;
    let mut i = 0;
    while i < SECRETS.len() {
        starts[SECRETS[i].0.as_bytes()[0] as usize] = true;
        i += 1;
    }
    starts
};
/// Written-out escapes a secret can follow in JSON or a URL, where the byte before it is a letter or digit.
const ESCAPES: &[&str] = &["\\n", "\\t", "\\r", "%20", "%0A", "%0a", "%0D", "%0d", "%09"];

pub fn redact(text: &str) -> (String, usize) {
    let (mut out, mut hidden, mut copied, mut i) = (String::new(), 0, 0, 0);
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let boundary = i == 0
            || !(bytes[i - 1].is_ascii_alphanumeric() || matches!(bytes[i - 1], b'_' | b'-'))
            || ESCAPES.iter().any(|e| text[..i].ends_with(e));
        if boundary
            && STARTS[bytes[i] as usize]
            && let Some((len, kind)) = secret(&text[i..])
        {
            out += &text[copied..i];
            out += &format!("[redacted {kind}]");
            (hidden, i) = (hidden + 1, i + len);
            copied = i;
            continue;
        }
        i += 1;
    }
    if hidden == 0 {
        return (text.into(), 0);
    }
    out += &text[copied..];
    (out, hidden)
}

fn secret(s: &str) -> Option<(usize, &'static str)> {
    if s.starts_with("-----BEGIN ")
        && s.lines().next().is_some_and(|l| l.contains("PRIVATE KEY") && l.ends_with("-----"))
    {
        let end = s.find("-----END ").and_then(|e| Some(e + 9 + s[e + 9..].find("-----")? + 5)).unwrap_or(s.len());
        return Some((end, "private key"));
    }
    SECRETS.iter().find_map(|&(prefix, kind, min, extra)| {
        let body = s.strip_prefix(prefix)?;
        let upper = prefix == "AKIA";
        let allowed = |c: char| c.is_ascii_alphanumeric() && !(upper && c.is_ascii_lowercase()) || extra.contains(c);
        let len = body.find(|c| !allowed(c)).unwrap_or(body.len());
        let keyish = !matches!(prefix, "sk-" | "oc_") || body[..len].bytes().any(|b| b.is_ascii_digit());
        (len >= min && keyish).then_some((prefix.len() + len, kind))
    })
}
