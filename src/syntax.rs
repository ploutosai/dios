/// Simple syntax highlighters that tokenize source text into styled spans.
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Language {
    Rust,
    Markdown,
    Plain,
}

impl Language {
    pub fn from_path(path: Option<&Path>, name: &str) -> Self {
        let ext = path
            .and_then(|p| p.extension())
            .and_then(|s| s.to_str())
            .or_else(|| Path::new(name).extension().and_then(|s| s.to_str()));
        match ext {
            Some("rs") => Language::Rust,
            Some("md" | "markdown") => Language::Markdown,
            _ => Language::Plain,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Markdown => "Markdown",
            Language::Plain => "",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenKind {
    Keyword,
    Type,
    String,
    Comment,
    Number,
    Punctuation,
    Macro,
    Lifetime,
    MdHeading,
    MdCode,
    MdLink,
    MdEmphasis,
    Plain,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
}

impl TokenKind {
    pub fn css_class(self) -> &'static str {
        match self {
            TokenKind::Keyword => "syn-kw",
            TokenKind::Type => "syn-ty",
            TokenKind::String => "syn-str",
            TokenKind::Comment => "syn-cmt",
            TokenKind::Number => "syn-num",
            TokenKind::Punctuation => "syn-punc",
            TokenKind::Macro => "syn-mac",
            TokenKind::Lifetime => "syn-lt",
            TokenKind::MdHeading => "syn-md-heading",
            TokenKind::MdCode => "syn-md-code",
            TokenKind::MdLink => "syn-md-link",
            TokenKind::MdEmphasis => "syn-md-em",
            TokenKind::Plain => "syn-plain",
        }
    }
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "yield",
];

const BUILTIN_TYPES: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8", "u16",
    "u32", "u64", "u128", "usize", "String", "Vec", "Option", "Result", "Box", "Rc", "Arc",
    "HashMap", "HashSet", "BTreeMap", "BTreeSet", "Some", "None", "Ok", "Err",
];

pub fn highlight(source: &str, language: Language) -> Vec<Vec<Token>> {
    puffin::profile_function!();
    match language {
        Language::Rust => highlight_rust(source),
        Language::Markdown => highlight_markdown(source),
        Language::Plain => highlight_plain(source),
    }
}

fn collect_lines(source: &str, mut f: impl FnMut(&str) -> Vec<Token>) -> Vec<Vec<Token>> {
    let mut result = Vec::new();
    for line in source.lines() {
        result.push(f(line));
    }
    if source.ends_with('\n') {
        result.push(vec![]);
    }
    if result.is_empty() {
        result.push(vec![]);
    }
    result
}

fn highlight_plain(source: &str) -> Vec<Vec<Token>> {
    collect_lines(source, |line| {
        if line.is_empty() {
            Vec::new()
        } else {
            vec![Token {
                kind: TokenKind::Plain,
                text: line.to_string(),
            }]
        }
    })
}

fn highlight_rust(source: &str) -> Vec<Vec<Token>> {
    let mut in_block_comment = false;
    collect_lines(source, |line| tokenize_line(line, &mut in_block_comment))
}

fn highlight_markdown(source: &str) -> Vec<Vec<Token>> {
    let mut in_fence = false;
    collect_lines(source, |line| tokenize_markdown_line(line, &mut in_fence))
}

fn push_plain(tokens: &mut Vec<Token>, text: &str) {
    if !text.is_empty() {
        tokens.push(Token {
            kind: TokenKind::Plain,
            text: text.to_string(),
        });
    }
}

fn tokenize_markdown_line(line: &str, in_fence: &mut bool) -> Vec<Token> {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();

    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        *in_fence = !*in_fence;
        let mut tokens = Vec::new();
        push_plain(&mut tokens, &line[..indent_len]);
        tokens.push(Token {
            kind: TokenKind::MdCode,
            text: trimmed.to_string(),
        });
        return tokens;
    }

    if *in_fence || line.starts_with("    ") || line.starts_with('\t') {
        return vec![Token {
            kind: TokenKind::MdCode,
            text: line.to_string(),
        }];
    }

    if trimmed.starts_with('#') {
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            let mut tokens = Vec::new();
            push_plain(&mut tokens, &line[..indent_len]);
            tokens.push(Token {
                kind: TokenKind::MdHeading,
                text: trimmed.to_string(),
            });
            return tokens;
        }
    }

    let mut tokens = Vec::new();
    push_plain(&mut tokens, &line[..indent_len]);
    let mut i = indent_len;

    if trimmed.starts_with('>') {
        tokens.push(Token {
            kind: TokenKind::Comment,
            text: ">".to_string(),
        });
        i += 1;
    } else if let Some(marker_len) = markdown_list_marker_len(trimmed) {
        tokens.push(Token {
            kind: TokenKind::Punctuation,
            text: trimmed[..marker_len].to_string(),
        });
        i += marker_len;
    }

    tokenize_markdown_inline(&line[i..], &mut tokens);
    tokens
}

fn markdown_list_marker_len(s: &str) -> Option<usize> {
    if s.starts_with("- ") || s.starts_with("* ") || s.starts_with("+ ") {
        return Some(2);
    }
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && s[digits..].starts_with(". ") {
        Some(digits + 2)
    } else {
        None
    }
}

fn tokenize_markdown_inline(s: &str, tokens: &mut Vec<Token>) {
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with('`') {
            let end = rest[1..]
                .find('`')
                .map(|n| i + 1 + n + 1)
                .unwrap_or(s.len());
            tokens.push(Token {
                kind: TokenKind::MdCode,
                text: s[i..end].to_string(),
            });
            i = end;
        } else if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end_paren) = rest[close + 2..].find(')') {
                    let end = i + close + 2 + end_paren + 1;
                    tokens.push(Token {
                        kind: TokenKind::MdLink,
                        text: s[i..end].to_string(),
                    });
                    i = end;
                    continue;
                }
            }
            push_plain(tokens, &s[i..i + 1]);
            i += 1;
        } else if rest.starts_with("**") || rest.starts_with("__") {
            let marker = &rest[..2];
            let end = rest[2..]
                .find(marker)
                .map(|n| i + 2 + n + 2)
                .unwrap_or(i + 2);
            tokens.push(Token {
                kind: TokenKind::MdEmphasis,
                text: s[i..end].to_string(),
            });
            i = end;
        } else if rest.starts_with('*') || rest.starts_with('_') {
            let marker = &rest[..1];
            let end = rest[1..]
                .find(marker)
                .map(|n| i + 1 + n + 1)
                .unwrap_or(i + 1);
            tokens.push(Token {
                kind: TokenKind::MdEmphasis,
                text: s[i..end].to_string(),
            });
            i = end;
        } else {
            let next = rest
                .find(|c| matches!(c, '`' | '[' | '*' | '_'))
                .map(|n| i + n)
                .unwrap_or(s.len());
            push_plain(tokens, &s[i..next]);
            i = next;
        }
    }
}

fn tokenize_line(line: &str, in_block_comment: &mut bool) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if *in_block_comment {
            let start = i;
            while i < len {
                if i + 1 < len && chars[i] == '*' && chars[i + 1] == '/' {
                    i += 2;
                    *in_block_comment = false;
                    break;
                }
                i += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Comment,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        let ch = chars[i];

        // Line comment
        if ch == '/' && i + 1 < len && chars[i + 1] == '/' {
            tokens.push(Token {
                kind: TokenKind::Comment,
                text: chars[i..].iter().collect(),
            });
            break;
        }

        // Block comment start
        if ch == '/' && i + 1 < len && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            *in_block_comment = true;
            while i < len {
                if i + 1 < len && chars[i] == '*' && chars[i + 1] == '/' {
                    i += 2;
                    *in_block_comment = false;
                    break;
                }
                i += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Comment,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        // String literal
        if ch == '"' {
            let start = i;
            i += 1;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            tokens.push(Token {
                kind: TokenKind::String,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        // Char literal or lifetime
        if ch == '\'' && i + 1 < len && chars.get(i + 1) != Some(&' ') {
            if i + 1 < len && chars[i + 1].is_alphabetic() && chars.get(i + 2) != Some(&'\'') {
                let start = i;
                i += 1;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Lifetime,
                    text: chars[start..i].iter().collect(),
                });
                continue;
            }
            let start = i;
            i += 1;
            while i < len && chars[i] != '\'' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            if i < len {
                i += 1;
            }
            tokens.push(Token {
                kind: TokenKind::String,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        // Numbers
        if ch.is_ascii_digit() {
            let start = i;
            if i + 1 < len && chars[i] == '0' && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                i += 2;
                while i < len && (chars[i].is_ascii_hexdigit() || chars[i] == '_') {
                    i += 1;
                }
            } else {
                while i < len && (chars[i].is_ascii_digit() || chars[i] == '_' || chars[i] == '.') {
                    i += 1;
                }
            }
            if i < len && (chars[i] == 'u' || chars[i] == 'i' || chars[i] == 'f') {
                i += 1;
                while i < len && (chars[i].is_ascii_digit() || chars[i].is_alphabetic()) {
                    i += 1;
                }
            }
            tokens.push(Token {
                kind: TokenKind::Number,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        // Identifiers and keywords
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();

            if i < len && chars[i] == '!' {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Macro,
                    text: format!("{word}!"),
                });
                continue;
            }

            let kind = if KEYWORDS.contains(&word.as_str()) {
                TokenKind::Keyword
            } else if BUILTIN_TYPES.contains(&word.as_str()) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };

            tokens.push(Token { kind, text: word });
            continue;
        }

        // Whitespace
        if ch.is_whitespace() {
            let start = i;
            while i < len && chars[i].is_whitespace() {
                i += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Plain,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        // Punctuation
        tokens.push(Token {
            kind: TokenKind::Punctuation,
            text: ch.to_string(),
        });
        i += 1;
    }

    tokens
}
