//! Parseur KeyValues (VDF) tolérant : appmanifest, libraryfolders, app_info_print.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Obj(Obj),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Obj(pub BTreeMap<String, Value>);

impl Obj {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(Value::Str(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn obj(&self, key: &str) -> Option<&Obj> {
        match self.0.get(key) {
            Some(Value::Obj(o)) => Some(o),
            _ => None,
        }
    }

    pub fn u64(&self, key: &str) -> u64 {
        self.str(key)
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
    }

    pub fn i64(&self, key: &str) -> i64 {
        self.str(key)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0)
    }

    pub fn strings(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().filter_map(|(k, v)| match v {
            Value::Str(s) => Some((k.as_str(), s.as_str())),
            Value::Obj(_) => None,
        })
    }

    pub fn objects(&self) -> impl Iterator<Item = (&str, &Obj)> {
        self.0.iter().filter_map(|(k, v)| match v {
            Value::Obj(o) => Some((k.as_str(), o)),
            Value::Str(_) => None,
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum VdfError {
    #[error("fin de fichier inattendue (ligne {0})")]
    UnexpectedEof(usize),
    #[error("accolade fermante inattendue (ligne {0})")]
    UnexpectedClose(usize),
    #[error("clé vide (ligne {0})")]
    EmptyKey(usize),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Str(String),
    Open(usize),
    Close(usize),
    Eof,
}

pub fn parse(input: &str) -> Result<Obj, VdfError> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let mut root = Obj::default();
    loop {
        match parser.peek() {
            Token::Eof => break,
            Token::Close(line) => return Err(VdfError::UnexpectedClose(*line)),
            _ => {}
        }
        let key = parser.expect_key()?;
        match parser.next() {
            Token::Open(_) => {
                let obj = parser.parse_obj()?;
                root.0.insert(key, Value::Obj(obj));
            }
            Token::Str(s) => {
                root.0.insert(key, Value::Str(s));
            }
            other => {
                let _ = other;
                return Err(VdfError::UnexpectedEof(0));
            }
        }
    }
    Ok(root)
}

/// Parse un document VDF dont la racine est un objet nommé (ex. "AppState").
pub fn parse_root(input: &str) -> Result<Obj, VdfError> {
    let mut obj = parse(input)?;
    if obj.0.len() == 1 {
        if let Some(Value::Obj(inner)) = obj.0.values().next().cloned() {
            return Ok(inner);
        }
    }
    if obj.0.len() == 1 && obj.0.contains_key("AppState") {
        if let Some(Value::Obj(inner)) = obj.0.remove("AppState") {
            return Ok(inner);
        }
    }
    Ok(obj)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn next(&mut self) -> Token {
        let token = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn expect_key(&mut self) -> Result<String, VdfError> {
        match self.next() {
            Token::Str(s) if s.is_empty() => Ok(s),
            Token::Str(s) => Ok(s),
            Token::Open(line) => Err(VdfError::EmptyKey(line)),
            Token::Close(line) => Err(VdfError::UnexpectedClose(line)),
            Token::Eof => Err(VdfError::UnexpectedEof(0)),
        }
    }

    fn parse_obj(&mut self) -> Result<Obj, VdfError> {
        let mut obj = Obj::default();
        loop {
            match self.peek().clone() {
                Token::Close(_) => {
                    self.next();
                    return Ok(obj);
                }
                Token::Eof => return Err(VdfError::UnexpectedEof(0)),
                _ => {}
            }
            let key = self.expect_key()?;
            match self.next() {
                Token::Open(_) => {
                    let inner = self.parse_obj()?;
                    obj.0.insert(key, Value::Obj(inner));
                }
                Token::Str(s) => {
                    obj.0.insert(key, Value::Str(s));
                }
                Token::Close(line) => return Err(VdfError::UnexpectedClose(line)),
                Token::Eof => return Err(VdfError::UnexpectedEof(0)),
            }
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, VdfError> {
    let bytes: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    while i < bytes.len() {
        let ch = bytes[i];
        match ch {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            '{' => {
                out.push(Token::Open(line));
                i += 1;
            }
            '}' => {
                out.push(Token::Close(line));
                i += 1;
            }
            '/' if i + 1 < bytes.len() && bytes[i + 1] == '/' => {
                while i < bytes.len() && bytes[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                let mut buf = String::new();
                while i < bytes.len() {
                    let c = bytes[i];
                    if c == '\\' && i + 1 < bytes.len() {
                        let next = bytes[i + 1];
                        match next {
                            'n' => buf.push('\n'),
                            't' => buf.push('\t'),
                            '\\' => buf.push('\\'),
                            '"' => buf.push('"'),
                            other => buf.push(other),
                        }
                        i += 2;
                        continue;
                    }
                    if c == '"' {
                        i += 1;
                        break;
                    }
                    if c == '\n' {
                        line += 1;
                    }
                    buf.push(c);
                    i += 1;
                }
                out.push(Token::Str(buf));
            }
            _ => {
                let mut buf = String::new();
                while i < bytes.len() {
                    let c = bytes[i];
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    buf.push(c);
                    i += 1;
                }
                out.push(Token::Str(buf));
            }
        }
    }
    out.push(Token::Eof);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPMANIFEST: &str = r#"
"AppState"
{
	"appid"		"228980"
	"Universe"		"1"
	"name"		"Steamworks Common Redistributables"
	"StateFlags"		"4"
	"installdir"		"Steamworks Shared"
	"SizeOnDisk"		"1029003611"
	"UserConfig"
	{
		"language"		"english"
	}
	"MountedDepots"
	{
		"228981"		"7155848575309492375"
	}
}
"#;

    #[test]
    fn parses_real_manifest() {
        let root = parse_root(APPMANIFEST).unwrap();
        assert_eq!(root.str("appid"), Some("228980"));
        assert_eq!(root.str("name"), Some("Steamworks Common Redistributables"));
        assert_eq!(root.u64("StateFlags"), 4);
        assert_eq!(root.u64("SizeOnDisk"), 1029003611);
        assert_eq!(root.obj("UserConfig").unwrap().str("language"), Some("english"));
        assert_eq!(
            root.obj("MountedDepots").unwrap().str("228981"),
            Some("7155848575309492375")
        );
    }

    #[test]
    fn parses_libraryfolders() {
        let text = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/var/home/x/.local/share/Steam"
		"label"		""
		"apps" { "10" "100" "20" "200" }
	}
	"1"
	{
		"path"		"/var/mnt/Jeux/Steam"
		"totalsize"		"1967847026688"
	}
}
"#;
        let root = parse_root(text).unwrap();
        let zero = root.obj("0").unwrap();
        assert_eq!(zero.str("path"), Some("/var/home/x/.local/share/Steam"));
        assert_eq!(root.obj("1").unwrap().u64("totalsize"), 1967847026688);
        assert_eq!(zero.obj("apps").unwrap().str("20"), Some("200"));
    }

    #[test]
    fn tolerates_comments_and_escapes() {
        let text = "// commentaire\n\"a\" \"x\\\"y\" // fin\n\"b\" { \"c\" \"d\" }";
        let root = parse_root(text).unwrap();
        assert_eq!(root.str("a"), Some("x\"y"));
        assert_eq!(root.obj("b").unwrap().str("c"), Some("d"));
    }

    #[test]
    fn unbalanced_braces_error() {
        assert!(parse_root("\"a\" { \"b\" \"c\"").is_err());
        assert!(parse_root("\"a\" }").is_err());
    }

    #[test]
    fn parses_app_info_print_shape() {
        let text = r#"
"228980"
{
	"common"
	{
		"name"		"Steamworks Common Redistributables"
		"type"		"Tool"
	}
	"config"
	{
		"launch"
		{
			"0"
			{
				"executable"		"installscript.vdf"
				"config" { "oslist" "linux" }
			}
		}
	}
}
"#;
        let root = parse_root(text).unwrap();
        let launch = root.obj("config").unwrap().obj("launch").unwrap();
        assert_eq!(
            launch.obj("0").unwrap().str("executable"),
            Some("installscript.vdf")
        );
    }
}
