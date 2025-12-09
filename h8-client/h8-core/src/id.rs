//! Human-readable ID generation (adjective-noun format).
//!
//! This module provides ID generation for emails using a memorable
//! adjective-noun format like "cold-lamp" or "blue-frog".

use crate::db::Database;
use crate::error::Result;

/// Word lists for ID generation.
pub struct WordLists {
    pub adjectives: Vec<String>,
    pub nouns: Vec<String>,
}

impl WordLists {
    /// Parse word lists from TOML content.
    pub fn from_toml(content: &str) -> Result<Self> {
        let parsed: toml::Value = content.parse()
            .map_err(|e| crate::error::Error::Config(format!("parsing word lists: {e}")))?;
        
        let adjectives = parsed
            .get("adjectives")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        
        let nouns = parsed
            .get("nouns")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        
        Ok(Self { adjectives, nouns })
    }

    /// Get embedded word lists (compiled into binary).
    pub fn embedded() -> Self {
        Self {
            adjectives: EMBEDDED_ADJECTIVES.iter().map(|s| s.to_string()).collect(),
            nouns: EMBEDDED_NOUNS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// ID generator that manages the pool of human-readable IDs.
pub struct IdGenerator<'a> {
    db: &'a Database,
}

impl<'a> IdGenerator<'a> {
    /// Create a new ID generator.
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Initialize the ID pool with word lists.
    pub fn init_pool(&self, words: &WordLists) -> Result<usize> {
        let adj_refs: Vec<&str> = words.adjectives.iter().map(|s| s.as_str()).collect();
        let noun_refs: Vec<&str> = words.nouns.iter().map(|s| s.as_str()).collect();
        self.db.seed_id_pool(&adj_refs, &noun_refs)
    }

    /// Allocate a new ID for a message.
    pub fn allocate(&self, remote_id: &str) -> Result<String> {
        self.db.allocate_id(remote_id)
    }

    /// Free an ID back to the pool.
    pub fn free(&self, short_id: &str) -> Result<bool> {
        self.db.free_id(short_id)
    }

    /// Look up the remote ID for a short ID.
    pub fn resolve(&self, short_id: &str) -> Result<Option<String>> {
        self.db.get_remote_by_id(short_id)
    }

    /// Look up the short ID for a remote ID.
    pub fn reverse_lookup(&self, remote_id: &str) -> Result<Option<String>> {
        self.db.get_id_by_remote(remote_id)
    }

    /// Get pool statistics.
    pub fn stats(&self) -> Result<IdPoolStats> {
        Ok(IdPoolStats {
            free: self.db.count_free_ids()?,
            used: self.db.count_used_ids()?,
        })
    }
}

/// ID pool statistics.
#[derive(Debug, Clone)]
pub struct IdPoolStats {
    pub free: usize,
    pub used: usize,
}

impl IdPoolStats {
    pub fn total(&self) -> usize {
        self.free + self.used
    }
}

/// Embedded adjectives (subset for compilation).
/// The full list is loaded from word_lists.toml at runtime.
const EMBEDDED_ADJECTIVES: &[&str] = &[
    "able", "acid", "aged", "airy", "akin", "alto", "amok", "anti", "arch", "arid",
    "arty", "auld", "avid", "away", "awol", "awry", "back", "bald", "bare", "base",
    "bass", "bats", "beat", "bent", "best", "beta", "bias", "blue", "bold", "bone",
    "bony", "boon", "born", "boss", "both", "brag", "buff", "bulk", "bush", "bust",
    "busy", "calm", "camp", "chic", "clad", "cold", "cool", "cosy", "cozy", "curt",
    "cute", "cyan", "daft", "damp", "dank", "dark", "deaf", "dear", "deep", "deft",
    "dire", "dirt", "done", "dour", "down", "drab", "dual", "dull", "dyed", "each",
    "east", "easy", "edgy", "epic", "even", "evil", "eyed", "fair", "fake", "fast",
    "faux", "fell", "fine", "firm", "five", "flat", "flip", "fond", "foul", "foxy",
    "free", "full", "gaga", "game", "gilt", "glad", "glib", "glum", "gold", "gone",
    "good", "gray", "grey", "grim", "hale", "half", "halt", "hard", "hazy", "held",
    "here", "hick", "high", "hind", "holy", "home", "huge", "iced", "icky", "idle",
    "iffy", "inky", "iron", "just", "keen", "kept", "kind", "lacy", "laid", "lame",
    "lank", "last", "late", "lazy", "lean", "left", "less", "lest", "like", "limp",
    "lite", "live", "loco", "lone", "long", "lost", "loud", "lush", "luxe", "made",
    "main", "male", "many", "mass", "maxi", "mean", "meek", "meet", "mere", "midi",
    "mild", "mini", "mint", "mock", "mono", "moot", "more", "most", "much", "must",
    "mute", "near", "neat", "next", "nice", "nigh", "nine", "none", "nosy", "nude",
    "null", "numb", "nuts", "oily", "okay", "only", "open", "oral", "oval", "over",
    "paid", "pale", "pass", "past", "pent", "pied", "pink", "plus", "poor", "port",
    "posh", "prim", "puff", "punk", "puny", "pure", "racy", "rank", "rare", "rash",
    "real", "rear", "rich", "rife", "ripe", "roan", "rosy", "rude", "rust", "safe",
    "salt", "same", "sane", "sear", "self", "sent", "sewn", "sham", "shed", "shot",
    "shut", "side", "sign", "size", "skew", "skim", "slim", "slow", "smug", "snub",
    "snug", "soft", "sold", "sole", "solo", "some", "sore", "sour", "sown", "spry",
    "star", "such", "sunk", "sure", "tall", "tame", "tart", "taut", "teal", "teen",
    "then", "thin", "tidy", "tied", "tiny", "toed", "tops", "torn", "trig", "trim",
    "true", "twin", "ugly", "used", "vain", "vast", "very", "vile", "void", "warm",
    "wary", "wavy", "waxy", "weak", "wide", "wild", "wily", "wise", "worn", "zany",
    "zero",
];

/// Embedded nouns (subset for compilation).
const EMBEDDED_NOUNS: &[&str] = &[
    "acid", "acre", "acts", "aged", "aide", "aims", "airs", "ally", "aloe", "alto",
    "amen", "amps", "ante", "anti", "ants", "apes", "apex", "aqua", "arch", "arcs",
    "area", "aria", "arms", "army", "arts", "atom", "aunt", "aura", "auto", "axes",
    "axis", "axle", "babe", "baby", "back", "bags", "bail", "bait", "bale", "ball",
    "balm", "band", "bane", "bang", "bank", "bans", "barb", "bark", "barn", "bars",
    "base", "bash", "bass", "bath", "bats", "bays", "bead", "beak", "beam", "bean",
    "bear", "beat", "beds", "beef", "beer", "bees", "beet", "bell", "belt", "bend",
    "bent", "best", "beta", "bets", "bias", "bids", "bike", "bill", "bind", "bins",
    "bird", "bite", "bits", "blob", "bloc", "blog", "blot", "blow", "blue", "blur",
    "boar", "boat", "body", "boil", "bold", "bolt", "bond", "bone", "book", "boom",
    "boon", "boot", "bore", "born", "boss", "bout", "bowl", "bows", "boys", "brag",
    "bran", "bras", "brat", "brew", "brig", "brim", "brit", "brow", "buck", "buds",
    "buff", "bugs", "bulb", "bulk", "bull", "bump", "bums", "bunk", "buns", "buoy",
    "burn", "burr", "bush", "bust", "buys", "buzz", "byte", "cabs", "cafe", "cage",
    "cake", "calf", "call", "calm", "camo", "camp", "cams", "cane", "cans", "cape",
    "caps", "card", "care", "carp", "cars", "cart", "case", "cash", "cast", "cats",
    "cave", "cell", "cent", "chap", "char", "chat", "chef", "chew", "chic", "chin",
    "chip", "chop", "cite", "city", "clam", "clan", "clap", "claw", "clay", "clip",
    "clot", "club", "clue", "coal", "coat", "coca", "coco", "code", "coil", "coin",
    "cola", "cold", "colt", "coma", "comb", "come", "comp", "cone", "cons", "cool",
    "coop", "cope", "cops", "copy", "cord", "core", "cork", "corn", "corp", "cost",
    "cosy", "coup", "cove", "cows", "cozy", "crab", "crew", "crib", "crop", "crow",
    "crux", "cube", "cubs", "cues", "cuff", "cult", "cups", "curb", "cure", "curl",
    "cusp", "cuts", "cyst", "dads", "dame", "damp", "dams", "dare", "dark", "darn",
    "dart", "dash", "data", "date", "days", "deaf", "deal", "dear", "debt", "deck",
    "deed", "deep", "deer", "deli", "demo", "dent", "desk", "dial", "dice", "dies",
    "diet", "digs", "dime", "ding", "dips", "dirt", "disc", "dish", "disk", "diva",
    "dive", "dock", "docs", "does", "dogs", "dole", "doll", "dome", "dong", "dons",
    "doom", "door", "dope", "dork", "dorm", "dose", "dots", "dove", "down", "drab",
    "drag", "draw", "drip", "drop", "drum", "dubs", "duck", "duct", "dude", "duel",
    "dues", "duet", "duff", "dump", "dune", "dung", "dunk", "dusk", "dust", "duty",
    "dyes", "dyke", "ears", "ease", "east", "eats", "echo", "edge", "eels", "eggs",
    "egos", "emir", "ends", "envy", "epic", "eras", "even", "evil", "exam", "exec",
    "exit", "expo", "eyes", "face", "fact", "fade", "fair", "fake", "fall", "fame",
    "fang", "fans", "fare", "farm", "fast", "fate", "fats", "fawn", "fear", "feat",
    "feds", "feed", "feel", "fees", "feet", "fell", "felt", "fern", "feud", "fife",
    "figs", "file", "fill", "film", "find", "fine", "fink", "fins", "fire", "firm",
    "fish", "fist", "fits", "five", "flag", "flak", "flap", "flat", "flaw", "flax",
    "flea", "flex", "flip", "flop", "flow", "flux", "foam", "foes", "foil", "fold",
    "folk", "font", "food", "fool", "foot", "fork", "form", "fort", "foul", "fowl",
    "frat", "fray", "free", "fret", "frog", "fuel", "full", "fund", "funk", "furs",
    "fury", "fuse", "fuss", "fuzz", "gage", "gags", "gain", "gait", "gala", "gale",
    "gall", "gals", "game", "gang", "gaps", "garb", "gasp", "gate", "gays", "gaze",
    "gear", "geek", "gems", "gent", "germ", "gets", "gift", "gigs", "gill", "gilt",
    "girl", "gist", "give", "glad", "glee", "glow", "glue", "goal", "goat", "gods",
    "goes", "gold", "golf", "gong", "good", "goon", "goth", "gout", "gown", "grab",
    "grad", "gran", "gray", "grey", "grid", "grin", "grip", "grit", "grub", "gulf",
    "gums", "guns", "gust", "guts", "guys", "gyms", "hack", "hail", "hair", "half",
    "hall", "halo", "halt", "hams", "hand", "hang", "hank", "hare", "harm", "harp",
    "hash", "hasp", "hate", "hats", "haul", "hawk", "haze", "hazy", "head", "heal",
    "heap", "heat", "heck", "heel", "heir", "held", "helm", "help", "hems", "herd",
    "here", "hero", "hick", "hide", "high", "hike", "hill", "hind", "hint", "hips",
    "hire", "hits", "hive", "hoax", "hobs", "hock", "hogs", "hold", "hole", "holy",
    "home", "hone", "honk", "hood", "hook", "hoop", "hope", "hops", "horn", "hose",
    "host", "hour", "hubs", "hues", "huff", "huge", "hugs", "hull", "hump", "hums",
    "hung", "hunk", "hunt", "hurt", "hush", "husk", "huts", "hymn", "hype", "iced",
    "icon", "idea", "idle", "idol", "iffy", "inch", "info", "inks", "inky", "inns",
    "into", "ions", "iris", "iron", "isle", "itch", "item", "jabs", "jack", "jade",
    "jail", "jams", "jars", "java", "jaws", "jazz", "jean", "jeer", "jell", "jerk",
    "jest", "jets", "jobs", "jock", "jogs", "join", "joke", "jolt", "jots", "jump",
    "june", "junk", "jury", "just", "keel", "keen", "keep", "kegs", "kelp", "kept",
    "kick", "kids", "kill", "kiln", "kilt", "kind", "king", "kiss", "kite", "kits",
    "knee", "knit", "knob", "knot", "know", "labs", "lace", "lack", "lacy", "lads",
    "lady", "lags", "laid", "lair", "lake", "lamb", "lame", "lamp", "land", "lane",
    "laps", "lard", "lark", "last", "late", "lava", "lawn", "laws", "lays", "lazy",
    "lead", "leaf", "leak", "lean", "leap", "left", "legs", "lend", "lens", "lent",
    "less", "liar", "lice", "lick", "lids", "lien", "lies", "life", "lift", "like",
    "limb", "lime", "limp", "line", "link", "lint", "lion", "lips", "list", "lite",
    "live", "load", "loaf", "loan", "lobe", "lobs", "lock", "loco", "loft", "logo",
    "logs", "lone", "long", "look", "loom", "loop", "loot", "lord", "lore", "lose",
    "loss", "lost", "lots", "loud", "lout", "love", "luck", "lull", "lump", "lung",
    "lure", "lurk", "lush", "lust", "luxe", "lynx",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_lists_embedded() {
        let words = WordLists::embedded();
        assert!(!words.adjectives.is_empty());
        assert!(!words.nouns.is_empty());
    }

    #[test]
    fn test_word_lists_from_toml() {
        let toml = r#"
        adjectives = ["cold", "blue"]
        nouns = ["lamp", "frog"]
        "#;
        let words = WordLists::from_toml(toml).unwrap();
        assert_eq!(words.adjectives, vec!["cold", "blue"]);
        assert_eq!(words.nouns, vec!["lamp", "frog"]);
    }

    #[test]
    fn test_id_generator() {
        let db = Database::open_memory().unwrap();
        let id_gen = IdGenerator::new(&db);
        
        let words = WordLists {
            adjectives: vec!["cold".to_string(), "blue".to_string()],
            nouns: vec!["lamp".to_string(), "frog".to_string()],
        };
        
        let count = id_gen.init_pool(&words).unwrap();
        assert_eq!(count, 4); // 2 * 2 = 4 (no overlap)
        
        let stats = id_gen.stats().unwrap();
        assert_eq!(stats.free, 4);
        assert_eq!(stats.used, 0);
        
        let id = id_gen.allocate("remote-1").unwrap();
        assert!(id.contains('-'));
        
        let stats = id_gen.stats().unwrap();
        assert_eq!(stats.free, 3);
        assert_eq!(stats.used, 1);
        
        let resolved = id_gen.resolve(&id).unwrap();
        assert_eq!(resolved, Some("remote-1".to_string()));
        
        let reverse = id_gen.reverse_lookup("remote-1").unwrap();
        assert_eq!(reverse, Some(id.clone()));
        
        id_gen.free(&id).unwrap();
        let stats = id_gen.stats().unwrap();
        assert_eq!(stats.free, 4);
        assert_eq!(stats.used, 0);
    }
}
