//! Steam 的 KeyValues（VDF/ACF）格式：`"key" "value"` 與 `"key" { ... }`。
//! 只用來讀 `libraryfolders.vdf` 和 `appmanifest_*.acf`。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Text(String),
    Section(Vec<(String, Node)>),
}

impl Node {
    /// 依序取子節點；鍵名不分大小寫（Steam 本身也不分）。
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Section(entries) => {
                entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
            }
            Node::Text(_) => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Node::Text(text) => Some(text),
            Node::Section(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Node)] {
        match self {
            Node::Section(entries) => entries,
            Node::Text(_) => &[],
        }
    }
}

fn tokens(text: &str) -> Result<Vec<String>, String> {
    // 以 "\u{0}{" / "\u{0}}" 標記大括號，避免和字串內容混淆。
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' => out.push(format!("\u{0}{c}")),
            '"' => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => return Err("unterminated string".into()),
                        },
                        Some(other) => s.push(other),
                        None => return Err("unterminated string".into()),
                    }
                }
                out.push(s);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            _ => return Err(format!("unexpected '{c}'")),
        }
    }
    Ok(out)
}

fn section(tokens: &[String], at: &mut usize, nested: bool) -> Result<Vec<(String, Node)>, String> {
    let mut entries = Vec::new();
    while *at < tokens.len() {
        let key = &tokens[*at];
        *at += 1;
        if key == "\u{0}}" {
            return if nested { Ok(entries) } else { Err("unexpected '}'".into()) };
        }
        if key.starts_with('\u{0}') {
            return Err("a section needs a key".into());
        }
        let value = tokens.get(*at).ok_or("missing value")?;
        *at += 1;
        let node = if value == "\u{0}{" {
            Node::Section(section(tokens, at, true)?)
        } else if value.starts_with('\u{0}') {
            return Err("unexpected '}'".into());
        } else {
            Node::Text(value.clone())
        };
        entries.push((key.clone(), node));
    }
    if nested { Err("missing '}'".into()) } else { Ok(entries) }
}

pub fn parse(text: &str) -> Result<Node, String> {
    let tokens = tokens(text)?;
    let mut at = 0;
    Ok(Node::Section(section(&tokens, &mut at, false)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_folders() {
        let text = r#""libraryfolders"
{
    "0"
    {
        "path"  "C:\\Program Files (x86)\\Steam"
        "apps" { "3450310" "15175623617" }
    }
}"#;
        let root = parse(text).unwrap();
        let first = root.get("LibraryFolders").unwrap().get("0").unwrap();
        assert_eq!(first.get("path").unwrap().text(), Some(r"C:\Program Files (x86)\Steam"));
        assert!(first.get("apps").unwrap().get("3450310").is_some());
    }

    #[test]
    fn broken_input_fails() {
        assert!(parse(r#""a" { "b" "c" "#).is_err());
        assert!(parse(r#""a" "#).is_err());
    }
}
