//! Splits a release name into words, separators and bracketed groups.

/// One piece of a release name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Token {
    /// A run of characters between separators.
    Word(String),
    /// A standalone `-`, as in `Title - 01`.
    Dash,
    /// A standalone `/`, between alternative titles.
    Slash,
    /// The contents of `[…]`, `(…)`, `{…}`, `【…】` or `「…」`.
    Bracket { open: char, content: String },
}

/// Whether a dot separates words, as in scene names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dots {
    /// `Title.S01E01.1080p`: dots separate words.
    Separate,
    /// `Kaiju No. 8`: dots belong to words.
    Keep,
}

/// Splits `text` into tokens.
///
/// With [`Dots::Separate`] a dot separates words unless it is a decimal point,
/// so `2.22` and `AAC2.0` stay whole. Everything after a standalone `|` is an
/// alternative title and is dropped.
pub(crate) fn tokenize(text: &str, dots: Dots) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if let Some((content, len)) = bracket_at(&chars, i) {
            flush(&mut word, &mut tokens);
            tokens.push(Token::Bracket { open: c, content });
            i += len;
            continue;
        }
        let dot_splits = dots == Dots::Separate && c == '.' && !is_decimal_point(&chars, i);
        if c.is_whitespace() || dot_splits {
            if flush(&mut word, &mut tokens) == Flushed::Pipe {
                return tokens;
            }
        } else {
            word.push(c);
        }
        i += 1;
    }
    flush(&mut word, &mut tokens);
    tokens
}

/// Whether the dot at `at` sits inside a number such as `2.22` or `DDP5.1`.
///
/// Digits on both sides aren't enough: in `S01E01.1080p` and `2016.1080p` the
/// digits after the dot run into a letter, so the dot separates words.
fn is_decimal_point(chars: &[char], at: usize) -> bool {
    let before = at.checked_sub(1).and_then(|i| chars.get(i));
    if !before.is_some_and(char::is_ascii_digit) {
        return false;
    }
    let mut after = chars.iter().skip(at + 1).skip_while(|c| c.is_ascii_digit());
    let digits = chars.get(at + 1).is_some_and(char::is_ascii_digit);
    digits && !after.next().is_some_and(|c| c.is_alphabetic())
}

/// The bracket opening at `start`: its contents and its length in chars.
fn bracket_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let close = match chars.get(start)? {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        '【' => '】',
        '「' => '」',
        '（' => '）',
        _ => return None,
    };
    let inner = chars.get(start + 1..)?;
    let len = inner.iter().position(|&c| c == close)?;
    let content = inner.get(..len)?.iter().collect();
    Some((content, len + 2))
}

#[derive(PartialEq, Eq)]
enum Flushed {
    Pipe,
    Other,
}

fn flush(word: &mut String, tokens: &mut Vec<Token>) -> Flushed {
    let token = match word.as_str() {
        "" => return Flushed::Other,
        "|" => {
            word.clear();
            return Flushed::Pipe;
        }
        "-" | "–" | "—" => Token::Dash,
        "/" | "／" => Token::Slash,
        _ => Token::Word(word.clone()),
    };
    word.clear();
    tokens.push(token);
    Flushed::Other
}
