//! `{placeholder}` expansion for filename patterns and command args.
//!
//! Known keys: `file`, `folder`, `filename`, `width`, `height`,
//! `datetime`, `date`, `time`. Unknown placeholders are left untouched.
//! `{{` and `}}` produce literal braces.

/// Key/value pairs used for expansion.
pub type Vars<'a> = [(&'a str, String)];

pub fn expand(template: &str, vars: &Vars<'_>) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(r) = tail.strip_prefix("{{") {
            out.push('{');
            rest = r;
        } else if let Some(r) = tail.strip_prefix("}}") {
            out.push('}');
            rest = r;
        } else if tail.starts_with('{') {
            match tail.find('}') {
                Some(end) => {
                    let key = &tail[1..end];
                    match vars.iter().find(|(k, _)| *k == key) {
                        Some((_, v)) => out.push_str(v),
                        None => out.push_str(&tail[..=end]),
                    }
                    rest = &tail[end + 1..];
                }
                None => {
                    out.push_str(tail);
                    rest = "";
                }
            }
        } else if let Some(r) = tail.strip_prefix('}') {
            out.push('}');
            rest = r;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_known_and_keeps_unknown() {
        let vars = [("file", "C:/a b/x.png".to_string()), ("width", "10".into())];
        assert_eq!(
            expand("{file} {width} {nope} {{lit}}", &vars),
            "C:/a b/x.png 10 {nope} {lit}"
        );
    }
}
