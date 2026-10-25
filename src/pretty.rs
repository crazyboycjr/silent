//! Minified JSON laid out one member per line. It re-indents the text as it
//! goes rather than parsing it whole, so the head of a long file, cut off in
//! the middle of a value, still reads, and key order stays as written.

/// Whether `text` is JSON written without line breaks, or JSON Lines with
/// long records: lines longer than people write by hand.
pub fn minified(text: &str) -> bool {
    let lines = text.lines().count().max(1);
    text.len() / lines > 300
}

/// `text` with two-space indents, at most `limit` bytes of it, cut at a
/// line's end. Also says whether it was cut.
pub fn json(text: &str, limit: usize) -> (String, bool) {
    let mut out = String::with_capacity(text.len().min(limit) + text.len().min(limit) / 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    // An opening bracket waits to see whether its container is empty.
    let mut opened = false;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n("  ", depth));
    };
    for c in text.chars() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        let closing = matches!(c, '}' | ']');
        if opened && !closing {
            newline(&mut out, depth);
        }
        let was_opened = std::mem::take(&mut opened);
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                depth += 1;
                opened = true;
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if !was_opened {
                    newline(&mut out, depth);
                }
                out.push(c);
                if depth == 0 {
                    out.push('\n');
                }
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            _ => out.push(c),
        }
        if out.len() > limit {
            let cut = out.as_bytes()[..limit]
                .iter()
                .rposition(|&b| b == b'\n')
                .unwrap_or(0);
            out.truncate(cut);
            return (out, true);
        }
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_and_arrays_open_up() {
        let (text, cut) = json(r#"{"a":1,"b":[true,null,"x, y"],"c":{},"d":[]}"#, 1000);
        assert!(!cut);
        assert_eq!(
            text,
            "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null,\n    \"x, y\"\n  ],\n  \"c\": {},\n  \"d\": []\n}\n"
        );
    }

    #[test]
    fn strings_keep_their_escapes() {
        let (text, _) = json(r#"{"q":"say \"hi\": {}"}"#, 1000);
        assert_eq!(text, "{\n  \"q\": \"say \\\"hi\\\": {}\"\n}\n");
    }

    #[test]
    fn json_lines_become_one_value_after_another() {
        let (text, _) = json("{\"a\":1}\n{\"a\":2}\n", 1000);
        assert_eq!(text, "{\n  \"a\": 1\n}\n{\n  \"a\": 2\n}\n");
    }

    #[test]
    fn a_head_cut_mid_value_still_lays_out() {
        let (text, cut) = json(r#"{"a":[1,2,{"b":"unfinis"#, 1000);
        assert!(!cut);
        assert!(text.ends_with("{\n      \"b\": \"unfinis"), "{text}");
    }

    #[test]
    fn output_stops_at_the_limit_on_a_line_end() {
        let long = format!("[{}]", vec!["1"; 1000].join(","));
        let (text, cut) = json(&long, 100);
        assert!(cut);
        assert!(text.len() <= 100);
        assert!(text.ends_with(','), "{text}");
    }

    #[test]
    fn hand_written_json_is_left_alone() {
        assert!(!minified("{\n  \"a\": 1\n}\n"));
        assert!(minified(&format!("{{\"k\":\"{}\"}}", "v".repeat(400))));
    }
}
