#[rustfmt::skip]
/// Alphanumeric characters (lowercase and upercase).
/// Roughly 6 bits of entropy per character.
static RANDOM_CHARACTERS: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
    'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M',
    'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

#[rustfmt::skip]
/// A set of characters to generate "readable" random strings, without vowels as to minimize the
/// chance of generating bad words :)
///
/// also 0, o, 1, l, 2, z, 5, s are removed, as they look similar and can be confused.
///
/// Roughly 5.6 bits of entropy per character.
static RANDOM_CHARACTERS_READABLE: &[char] = &[
    '3', '4', '6', '7', '8', '9',
    'b', 'c', 'd', 'f', 'g', 'h', 'j', 'k', 'm', 'n', 'p', 'q', 'r', 't', 'v', 'w', 'x', 'y',
    'B', 'C', 'D', 'F', 'G', 'H', 'J', 'K', 'M', 'N', 'P', 'Q', 'R', 'T', 'V', 'W', 'X', 'Y',
    '!', '@', '#', '$', '%', '&',
];

/// Generate a random string with N characters.
fn random_string_with_characters(length: usize, characters: &[char]) -> String {
    use rand::Rng;

    let mut rng = rand::thread_rng();
    let distribution = rand::distributions::Uniform::new(0, characters.len());

    let mut string = String::with_capacity(length);
    for _ in 0..length {
        let idx = rng.sample(distribution);
        string.push(characters[idx])
    }

    string
}

pub fn string(length: usize) -> String {
    random_string_with_characters(length, RANDOM_CHARACTERS)
}

pub fn readable_string(length: usize) -> String {
    random_string_with_characters(length, RANDOM_CHARACTERS_READABLE)
}

/// Generate a password.
pub fn password() -> String {
    const PASSWORD_LENGTH: usize = 24;
    readable_string(PASSWORD_LENGTH)
}