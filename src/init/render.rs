//! Tiny brace-substitution renderer for embedded `dairo init` templates.
//!
//! Templates contain `{{ key }}` placeholders (whitespace-tolerant) that are
//! replaced with values from a variable map. There are deliberately no
//! conditionals or loops: variation across frameworks is expressed by shipping
//! separate template files, not by logic inside templates. That keeps the engine
//! a few dozen lines and avoids pulling in a heavyweight template dependency,
//! matching this codebase's "build files programmatically" precedent.

use std::collections::BTreeMap;

/// Renders `template`, replacing every `{{ key }}` occurrence with the matching
/// value from `vars`. Whitespace inside the braces is ignored, so `{{key}}`,
/// `{{ key }}`, and `{{  key  }}` are equivalent. An unknown key is left
/// verbatim (the `{{ ... }}` is preserved) so a typo is visible in the output
/// rather than silently producing an empty string.
pub fn render(template: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(close) = template[i + 2..].find("}}") {
                let raw_key = &template[i + 2..i + 2 + close];
                let key = raw_key.trim();
                if let Some(value) = vars.get(key) {
                    out.push_str(value);
                    i = i + 2 + close + 2;
                    continue;
                }
                // Unknown placeholder: emit it unchanged so the mistake surfaces.
                out.push_str(&template[i..i + 2 + close + 2]);
                i = i + 2 + close + 2;
                continue;
            }
        }
        // Copy a single UTF-8 char so we never split a multi-byte sequence.
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Length in bytes of the UTF-8 char that starts with `first_byte`.
fn utf8_char_len(first_byte: u8) -> usize {
    match first_byte {
        b if b < 0x80 => 1,
        b if b >> 5 == 0b110 => 2,
        b if b >> 4 == 0b1110 => 3,
        b if b >> 3 == 0b11110 => 4,
        // Continuation byte (shouldn't start a char): advance one to make
        // progress rather than loop forever.
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
        pairs
            .iter()
            .map(|(k, v)| (*k, v.to_string()))
            .collect::<BTreeMap<_, _>>()
    }

    #[test]
    fn substitutes_simple_placeholder() {
        let out = render("hello {{name}}!", &vars(&[("name", "world")]));
        assert_eq!(out, "hello world!");
    }

    #[test]
    fn tolerates_internal_whitespace() {
        let out = render(
            "a={{ a }} b={{  b  }} c={{c}}",
            &vars(&[("a", "1"), ("b", "2"), ("c", "3")]),
        );
        assert_eq!(out, "a=1 b=2 c=3");
    }

    #[test]
    fn repeats_same_key() {
        let out = render("{{x}}-{{x}}-{{x}}", &vars(&[("x", "z")]));
        assert_eq!(out, "z-z-z");
    }

    #[test]
    fn leaves_unknown_placeholder_verbatim() {
        let out = render("a={{known}} b={{unknown}}", &vars(&[("known", "1")]));
        assert_eq!(out, "a=1 b={{unknown}}");
    }

    #[test]
    fn leaves_unterminated_braces_verbatim() {
        let out = render("trailing {{ oops", &vars(&[("oops", "x")]));
        assert_eq!(out, "trailing {{ oops");
    }

    #[test]
    fn handles_multibyte_content() {
        let out = render("café {{x}} ☕", &vars(&[("x", "—")]));
        assert_eq!(out, "café — ☕");
    }

    #[test]
    fn empty_value_replaces_placeholder() {
        let out = render("[{{x}}]", &vars(&[("x", "")]));
        assert_eq!(out, "[]");
    }
}
