//! Random value answerer, ported from upstream's 'searx/answerers/random.py'
//! (plus the "random number", "coin", "dice" and "list" variants of the older
//! SearX releases).
//!
//! Upstream uses Python's 'random' module and 'uuid.uuid4()'.  This port brings
//! a small xorshift64* generator (the crate has no 'rand' dependency); the
//! 'uuid' / 'sha256' / 'color' formats match upstream, the string alphabet is
//! the same as 'string.ascii_lowercase + string.digits + string.ascii_uppercase'.
//!
//! The Lead declares 'pub mod answerers;' in 'src/lib.rs'.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use super::{AnswerContext, Answerer};

/// The keyword of the answerer.
pub const KEYWORDS: &[&str] = &["random"];

/// The random types the answerer knows (upstream 'random_types' + the older
/// variants).
pub const TYPES: &[&str] = &[
    "int", "number", "float", "string", "sha256", "uuid", "color", "coin", "dice", "list",
];

/// The alphabet of 'random string' (upstream 'random_characters()').
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The random value answerer.
pub struct Random;

impl Answerer for Random {
    fn id(&self) -> &'static str {
        "random"
    }
    fn name(&self) -> &'static str {
        "Random value generator"
    }
    fn description(&self) -> &'static str {
        "Generate different random values"
    }
    fn keywords(&self) -> &'static [&'static str] {
        KEYWORDS
    }
    fn answer(&self, query: &str, _context: &AnswerContext) -> Vec<String> {
        let mut parts = query.split_whitespace();
        let _keyword = parts.next();
        let Some(kind) = parts.next() else {
            return Vec::new();
        };
        let arguments: Vec<&str> = parts.collect();
        let mut rng = Rng::from_entropy();
        match random_value(&kind.to_lowercase(), &arguments, &mut rng) {
            Some(value) => vec![value],
            None => Vec::new(),
        }
    }
}

/// A small, dependency free random number generator (xorshift64*).
pub struct Rng {
    state: u64,
}

impl Rng {
    /// A generator with a fixed seed (mostly for the tests).
    pub fn with_seed(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// A generator seeded from the clock, a process wide counter and the stack
    /// address (enough for an instant answer, not for cryptography).
    pub fn from_entropy() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let address = &nanos as *const u64 as u64;
        Self::with_seed(nanos ^ counter.rotate_left(32) ^ address)
    }

    /// The next 64 bit value.
    pub fn next_u64(&mut self) -> u64 {
        let mut state = self.state;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.state = state;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A uniform value in '0..bound' (rejection sampling, no modulo bias).
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        let zone = u64::MAX - (u64::MAX % bound);
        loop {
            let value = self.next_u64();
            if value < zone {
                return value % bound;
            }
        }
    }

    /// A uniform value in the inclusive range 'low..=high'.
    pub fn range(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        let span = (high - low + 1) as u64;
        low + self.below(span) as i64
    }
}

/// One random value of 'kind'; 'None' when the type is unknown (or "list" was
/// used without an item to pick).
pub fn random_value(kind: &str, arguments: &[&str], rng: &mut Rng) -> Option<String> {
    match kind {
        "int" | "number" => Some(rng.range(-(2i64.pow(31)), 2i64.pow(31)).to_string()),
        "float" => Some(format!(
            "{}",
            (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
        )),
        "string" => Some(random_string(rng)),
        "sha256" => {
            let mut hasher = Sha256::new();
            hasher.update(random_string(rng).as_bytes());
            let digest = hasher.finalize();
            Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
        }
        "uuid" => Some(uuid_v4(rng)),
        "color" => Some(format!("#{:06X}", rng.below(0x0100_0000))),
        "coin" => Some(if rng.below(2) == 0 { "Heads" } else { "Tails" }.to_string()),
        "dice" => {
            let sides = arguments
                .first()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|sides| (2..=1000).contains(sides))
                .unwrap_or(6);
            Some(rng.range(1, sides as i64).to_string())
        }
        "list" | "pick" => {
            if arguments.is_empty() {
                return None;
            }
            let index = rng.below(arguments.len() as u64) as usize;
            Some(arguments[index].to_string())
        }
        _ => None,
    }
}

/// A random string (upstream picks 8 to 32 characters).
pub fn random_string(rng: &mut Rng) -> String {
    let length = rng.range(8, 32) as usize;
    (0..length)
        .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize] as char)
        .collect()
}

/// A random UUID version 4, formatted like Python's 'uuid.uuid4()'.
pub fn uuid_v4(rng: &mut Rng) -> String {
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        chunk.copy_from_slice(&rng.next_u64().to_ne_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> AnswerContext {
        AnswerContext::default()
    }

    #[test]
    fn plugin_metadata_matches_upstream() {
        let answerer = Random;
        assert_eq!(answerer.id(), "random");
        assert_eq!(answerer.name(), "Random value generator");
        assert_eq!(answerer.keywords(), KEYWORDS);
        assert!(TYPES.contains(&"dice"));
    }

    #[test]
    fn the_generator_is_deterministic_and_uniform_enough() {
        let mut first = Rng::with_seed(42);
        let mut second = Rng::with_seed(42);
        let mut other = Rng::with_seed(43);
        let left: Vec<u64> = (0..8).map(|_| first.next_u64()).collect();
        let right: Vec<u64> = (0..8).map(|_| second.next_u64()).collect();
        assert_eq!(left, right);
        assert_ne!(left[0], other.next_u64());
        let mut rng = Rng::with_seed(7);
        assert_eq!(rng.below(1), 0);
        for _ in 0..1000 {
            assert!(rng.below(10) < 10);
        }
        for _ in 0..1000 {
            let value = rng.range(-5, 5);
            assert!((-5..=5).contains(&value));
        }
        assert_eq!(Rng::with_seed(1).range(3, 3), 3);
        assert!(Rng::with_seed(0).next_u64() != 0);
    }

    #[test]
    fn random_types_produce_the_expected_shapes() {
        let mut rng = Rng::with_seed(1234);
        let number: i64 = random_value("int", &[], &mut rng).unwrap().parse().unwrap();
        assert!((-2i64.pow(31)..=2i64.pow(31)).contains(&number));
        assert!(!random_value("number", &[], &mut rng).unwrap().is_empty());
        let float: f64 = random_value("float", &[], &mut rng)
            .unwrap()
            .parse()
            .unwrap();
        assert!((0.0..1.0).contains(&float));
        let string = random_value("string", &[], &mut rng).unwrap();
        assert!((8..=32).contains(&string.len()));
        assert!(string.chars().all(|c| c.is_ascii_alphanumeric()));
        let hash = random_value("sha256", &[], &mut rng).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        let color = random_value("color", &[], &mut rng).unwrap();
        assert!(color.starts_with('#') && color.len() == 7);
        assert!(matches!(
            random_value("coin", &[], &mut rng).unwrap().as_str(),
            "Heads" | "Tails"
        ));
        assert!(random_value("unknown", &[], &mut rng).is_none());
    }

    #[test]
    fn dice_are_rolled_within_their_sides() {
        let mut rng = Rng::with_seed(99);
        for _ in 0..200 {
            let roll: i64 = random_value("dice", &[], &mut rng)
                .unwrap()
                .parse()
                .unwrap();
            assert!((1..=6).contains(&roll));
        }
        for _ in 0..200 {
            let roll: i64 = random_value("dice", &["20"], &mut rng)
                .unwrap()
                .parse()
                .unwrap();
            assert!((1..=20).contains(&roll));
        }
        // An invalid number of sides falls back to a six sided dice.
        let roll: i64 = random_value("dice", &["1"], &mut rng)
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=6).contains(&roll));
    }

    #[test]
    fn list_picks_one_of_the_arguments() {
        let mut rng = Rng::with_seed(5);
        for _ in 0..50 {
            let picked = random_value("list", &["a", "b", "c"], &mut rng).unwrap();
            assert!(["a", "b", "c"].contains(&picked.as_str()));
        }
        assert!(random_value("list", &[], &mut rng).is_none());
    }

    #[test]
    fn uuids_are_version_4() {
        let mut rng = Rng::with_seed(2024);
        for _ in 0..20 {
            let uuid = uuid_v4(&mut rng);
            assert_eq!(uuid.len(), 36);
            let parts: Vec<&str> = uuid.split('-').collect();
            assert_eq!(
                parts.iter().map(|part| part.len()).collect::<Vec<usize>>(),
                vec![8, 4, 4, 4, 12]
            );
            assert!(uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            assert_eq!(parts[2].as_bytes()[0], b'4');
            assert!(matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
        }
    }

    #[test]
    fn the_answerer_answers_the_random_keyword() {
        let answerer = Random;
        let answers = answerer.answer("random uuid", &context());
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].len(), 36);
        assert!(answerer.answer("random", &context()).is_empty());
        assert!(answerer.answer("random unknown", &context()).is_empty());
        assert!(answerer.answer("uuid", &context()).is_empty());
    }
}
