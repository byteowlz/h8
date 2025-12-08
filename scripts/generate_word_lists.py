#!/usr/bin/env python3
"""Generate word lists for human-readable email IDs.

Uses NLTK WordNet for part-of-speech tagging and wordfreq for
filtering to common, well-known words.
"""

from pathlib import Path

from nltk.corpus import wordnet as wn
from wordfreq import iter_wordlist, word_frequency

# Configuration
WORD_LENGTH = 4
MIN_FREQ_ADJ = 5e-7  # Slightly lower threshold for adjectives (fewer in English)
MIN_FREQ_NOUN = 1e-6  # Standard threshold for nouns
OUTPUT_PATH = Path(__file__).parent.parent / "h8" / "data" / "word_lists.toml"

# Common adjectives to include regardless of WordNet classification
EXTRA_ADJECTIVES = {
    "able",
    "aged",
    "airy",
    "arty",
    "avid",
    "away",
    "bare",
    "base",
    "best",
    "blue",
    "bold",
    "bony",
    "both",
    "bulk",
    "busy",
    "calm",
    "chic",
    "cold",
    "cool",
    "cozy",
    "cute",
    "damp",
    "dank",
    "dark",
    "deaf",
    "dear",
    "deep",
    "deft",
    "done",
    "dour",
    "drab",
    "dual",
    "dull",
    "each",
    "easy",
    "edgy",
    "even",
    "evil",
    "fair",
    "fake",
    "fast",
    "fine",
    "firm",
    "flat",
    "fond",
    "foul",
    "foxy",
    "free",
    "full",
    "game",
    "gilt",
    "glad",
    "glib",
    "glum",
    "gold",
    "good",
    "gray",
    "grey",
    "grim",
    "half",
    "hard",
    "hazy",
    "high",
    "holy",
    "huge",
    "iced",
    "idle",
    "inky",
    "iron",
    "just",
    "keen",
    "kind",
    "lacy",
    "lame",
    "lank",
    "last",
    "late",
    "lazy",
    "lean",
    "left",
    "like",
    "limp",
    "lite",
    "live",
    "loco",
    "lone",
    "long",
    "lost",
    "loud",
    "lush",
    "made",
    "main",
    "many",
    "mass",
    "mean",
    "meek",
    "mere",
    "mild",
    "mini",
    "more",
    "most",
    "much",
    "mute",
    "near",
    "neat",
    "next",
    "nice",
    "numb",
    "oily",
    "okay",
    "only",
    "open",
    "oral",
    "oval",
    "over",
    "paid",
    "pale",
    "past",
    "pink",
    "plus",
    "posh",
    "poor",
    "pure",
    "rare",
    "rash",
    "real",
    "rear",
    "rich",
    "ripe",
    "rosy",
    "rude",
    "safe",
    "sage",
    "same",
    "sane",
    "shut",
    "slim",
    "slow",
    "smug",
    "snug",
    "soft",
    "sole",
    "some",
    "sore",
    "sour",
    "spry",
    "such",
    "sure",
    "tall",
    "tame",
    "tart",
    "taut",
    "thin",
    "tidy",
    "tiny",
    "torn",
    "trig",
    "trim",
    "true",
    "twin",
    "ugly",
    "used",
    "vain",
    "vast",
    "very",
    "warm",
    "wary",
    "wavy",
    "weak",
    "wide",
    "wild",
    "wily",
    "wise",
    "worn",
    "zany",
    "zero",
}

# Words to exclude (offensive, confusing, or inappropriate)
EXCLUDED_WORDS = {
    # Potentially offensive
    "anal",
    "anus",
    "butt",
    "cock",
    "crap",
    "damn",
    "dick",
    "dumb",
    "fart",
    "hell",
    "jerk",
    "pimp",
    "piss",
    "poop",
    "porn",
    "sexy",
    "slut",
    "turd",
    # Violence/negative
    "bomb",
    "dead",
    "drug",
    "gore",
    "gory",
    "hate",
    "hurt",
    "kill",
    "lust",
    "nazi",
    "lewd",
    "perv",
    "sick",
    "death",
    # Confusing homophones
    "fore",
    "four",
    "knew",
    "know",
    "knot",
    "pare",
    "pair",
    "pear",
    "their",
    "there",
    "they",
    "ware",
    "wear",
    "were",
    "whom",
    "yore",
    "your",
    # Too short or common function words
    "for",
    "not",
    "too",
    "two",
    "the",
    "and",
    "but",
    "yet",
    # Medical/body that might be awkward
    "ache",
    "bile",
    "puke",
    "snot",
    "spit",
    "urea",
    # Roman numerals and abbreviations (not real words)
    "viii",
    "xiii",
    "xvii",
    "inst",
    "adhd",
    # Proper nouns / names (not suitable for IDs)
    "abel",
    "adam",
    "aden",
    "ajax",
    "alps",
    "amos",
    "anna",
    "anne",
    "arab",
    "asia",
    "baja",
    "bali",
    "beck",
    "bert",
    "beth",
    "bonn",
    "bose",
    "brad",
    "burt",
    "cain",
    "carl",
    "chad",
    "chen",
    "chip",
    "chow",
    "cobb",
    "coke",
    "cole",
    "como",
    "cook",
    "cuba",
    "dale",
    "dana",
    "dane",
    "dave",
    "dawn",
    "dean",
    "dell",
    "deng",
    "devi",
    "dion",
    "doug",
    "drew",
    "duke",
    "earl",
    "eden",
    "emma",
    "erik",
    "erma",
    "euro",
    "evan",
    "ewan",
    "ezra",
    "faye",
    "fiat",
    "fiji",
    "finn",
    "ford",
    "fran",
    "fred",
    "gabe",
    "gaia",
    "gary",
    "gaza",
    "gene",
    "glen",
    "gram",
    "greg",
    "gulf",
    "hans",
    "hart",
    "hess",
    "hewn",
    "holt",
    "hong",
    "hood",
    "hope",
    "hugo",
    "hull",
    "hunt",
    "ibis",
    "igor",
    "inca",
    "indo",
    "iowa",
    "iran",
    "iraq",
    "iris",
    "isle",
    "ivan",
    "jack",
    "jade",
    "jain",
    "jake",
    "jane",
    "java",
    "jean",
    "jedi",
    "jeff",
    "jill",
    "joel",
    "john",
    "jose",
    "josh",
    "juan",
    "jude",
    "judy",
    "kane",
    "karl",
    "kate",
    "kent",
    "khan",
    "kiev",
    "kirk",
    "klan",
    "koch",
    "kong",
    "kurt",
    "kyle",
    "lara",
    "lars",
    "lena",
    "leon",
    "levy",
    "liam",
    "lima",
    "lisa",
    "lois",
    "lola",
    "luke",
    "luna",
    "lynn",
    "macy",
    "mali",
    "marc",
    "mari",
    "mark",
    "mars",
    "marx",
    "mary",
    "matt",
    "mead",
    "mega",
    "mesa",
    "mica",
    "mike",
    "ming",
    "mist",
    "modi",
    "mont",
    "muse",
    "musk",
    "myra",
    "nasa",
    "nato",
    "navy",
    "neal",
    "neil",
    "nero",
    "ness",
    "nick",
    "nike",
    "nina",
    "noah",
    "noel",
    "nora",
    "nova",
    "numa",
    "oman",
    "omar",
    "opec",
    "opus",
    "oslo",
    "otto",
    "owen",
    "pace",
    "pact",
    "papa",
    "para",
    "paul",
    "peck",
    "penn",
    "peru",
    "pete",
    "phil",
    "pisa",
    "pius",
    "pius",
    "polk",
    "polo",
    "pope",
    "ravi",
    "reid",
    "rene",
    "rhea",
    "rico",
    "riga",
    "rita",
    "ritz",
    "roma",
    "rome",
    "rosa",
    "rose",
    "ross",
    "roth",
    "ruby",
    "rudy",
    "rush",
    "ruth",
    "ryan",
    "saba",
    "sage",
    "said",
    "sara",
    "saul",
    "scot",
    "sean",
    "sega",
    "seth",
    "shah",
    "shaw",
    "siam",
    "sikh",
    "sony",
    "span",
    "stan",
    "suez",
    "sufi",
    "taft",
    "tara",
    "tate",
    "thai",
    "theo",
    "thor",
    "tina",
    "tito",
    "todd",
    "togo",
    "tony",
    "troy",
    "ucla",
    "unix",
    "urea",
    "usda",
    "utah",
    "vida",
    "vila",
    "visa",
    "wade",
    "walt",
    "wang",
    "ward",
    "watt",
    "webb",
    "weed",
    "well",
    "west",
    "whig",
    "wiki",
    "wilt",
    "wins",
    "witt",
    "wolf",
    "wong",
    "wood",
    "wouk",
    "xmas",
    "yale",
    "yang",
    "yell",
    "yoga",
    "yogi",
    "york",
    "yuan",
    "yves",
    "zack",
    "zeke",
    "zeus",
    "zion",
    "zulu",
}


def is_adjective(word: str) -> bool:
    """Check if word can be used as an adjective in WordNet."""
    synsets = wn.synsets(word, pos=wn.ADJ)
    return len(synsets) > 0


def is_noun(word: str) -> bool:
    """Check if word can be used as a noun in WordNet."""
    synsets = wn.synsets(word, pos=wn.NOUN)
    return len(synsets) > 0


def get_adjectives() -> set[str]:
    """Get 4-letter adjectives from wordfreq, validated by WordNet."""
    adjectives = set()

    # Iterate through most common English words
    for word in iter_wordlist("en"):
        if len(word) != WORD_LENGTH:
            continue
        if not word.isalpha():
            continue
        if word in EXCLUDED_WORDS:
            continue
        if word_frequency(word, "en") < MIN_FREQ_ADJ:
            break  # wordfreq list is sorted by frequency, so we can stop
        if is_adjective(word):
            adjectives.add(word)

    # Add common adjectives that might be missed
    for word in EXTRA_ADJECTIVES:
        if word not in EXCLUDED_WORDS:
            adjectives.add(word)

    return adjectives


def get_nouns() -> set[str]:
    """Get 4-letter nouns from wordfreq, validated by WordNet."""
    nouns = set()

    # Iterate through most common English words
    for word in iter_wordlist("en"):
        if len(word) != WORD_LENGTH:
            continue
        if not word.isalpha():
            continue
        if word in EXCLUDED_WORDS:
            continue
        if word_frequency(word, "en") < MIN_FREQ_NOUN:
            break  # wordfreq list is sorted by frequency, so we can stop
        if is_noun(word):
            nouns.add(word)

    return nouns


def format_word_list(words: list[str], indent: str = "    ") -> str:
    """Format word list as TOML array with 10 words per line."""
    lines = []
    sorted_words = sorted(words)
    for i in range(0, len(sorted_words), 10):
        chunk = sorted_words[i : i + 10]
        line = ", ".join(f'"{w}"' for w in chunk)
        lines.append(f"{indent}{line},")
    return "\n".join(lines)


def main():
    print("Fetching adjectives from WordNet...")
    adjectives = get_adjectives()
    print(f"  Found {len(adjectives)} adjectives")

    print("Fetching nouns from WordNet...")
    nouns = get_nouns()
    print(f"  Found {len(nouns)} nouns")

    # Note: overlap is allowed - the ID generator will avoid same-word combinations
    overlap = adjectives & nouns
    print(f"  Overlap (words in both lists): {len(overlap)}")

    print(f"\nFinal counts:")
    print(f"  Adjectives: {len(adjectives)}")
    print(f"  Nouns: {len(nouns)}")
    print(f"  Total combinations: {len(adjectives) * len(nouns):,}")

    # Effective combinations (excluding same-word pairs like "cold-cold")
    effective = len(adjectives) * len(nouns) - len(overlap)

    # Generate TOML content
    toml_content = f"""# Word lists for generating human-readable email IDs
# Format: <adjective>-<noun> (e.g., "cold-lamp", "blue-frog")
#
# Generated by scripts/generate_word_lists.py
# Source: NLTK WordNet + wordfreq frequency filtering
#
# Adjectives: {len(adjectives)}
# Nouns: {len(nouns)}
# Overlap: {len(overlap)} (words appearing in both lists)
# Effective combinations: {effective:,} (excluding same-word pairs)

adjectives = [
{format_word_list(list(adjectives))}
]

nouns = [
{format_word_list(list(nouns))}
]
"""

    OUTPUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT_PATH.write_text(toml_content)
    print(f"\nWritten to {OUTPUT_PATH}")

    # Print some example IDs
    import random

    adj_list = list(adjectives)
    noun_list = list(nouns)
    print("\nExample IDs:")
    for _ in range(10):
        adj = random.choice(adj_list)
        noun = random.choice(noun_list)
        print(f"  {adj}-{noun}")


if __name__ == "__main__":
    main()
