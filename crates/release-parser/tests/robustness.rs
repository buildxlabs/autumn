//! `parse` never panics and keeps what it can read from unreadable names.

use release_parser::parse;

/// The characters release names are made of, weighted toward the tricky ones.
const ALPHABET: &[char] = &[
    'a', 'B', 'e', 'E', 'p', 'P', 'S', 's', 'v', 'x', 'O', 'V', 'A', '0', '1', '2', '9', ' ', ' ',
    '.', '_', '-', '~', '/', '|', ':', '+', '[', ']', '(', ')', '{', '}', '【', '】', '「', '」',
    '（', '）', '第', '話', '話', 'é', 'Я', '＊', '\u{200b}', '\t',
];

/// A small xorshift generator, so the test needs no dependency and is reproducible.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `bound`, which must be non-zero.
    fn below(&mut self, bound: usize) -> usize {
        let bound = u64::try_from(bound).unwrap_or(u64::MAX);
        usize::try_from(self.next() % bound).unwrap_or_default()
    }
}

#[test]
fn random_strings_never_panic() {
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let len = rng.below(60);
        let name: String = (0..len)
            .filter_map(|_| ALPHABET.get(rng.below(ALPHABET.len())))
            .collect();
        let _ = parse(&name);
        let _ = parse(&format!("{name}.mkv"));
    }
}

#[test]
fn long_digit_runs_and_markers_never_panic() {
    for name in [
        "99999999999999999999",
        "Title - 99999999999999999999",
        "S99999999999E99999999999",
        "Title - 01-99999999999",
        "Title S01-S99999999999",
        "Title Vol.99999999999",
        "[",
        "[]",
        "( ( ( (",
        "- - - -",
        ".mkv",
        "",
    ] {
        let _ = parse(name);
    }
}

#[test]
fn an_unreadable_name_still_gives_its_title() {
    for (name, title) in [
        (
            "Sousou no Frieren ??? ### !!!",
            "Sousou no Frieren ??? ### !!!",
        ),
        (
            "[Group] Sousou no Frieren - ### [@@@]",
            "Sousou no Frieren - ###",
        ),
        (
            "Sousou no Frieren ][ )( - }{",
            "Sousou no Frieren ][ )( - }{",
        ),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some(title), "{name}");
    }
}
