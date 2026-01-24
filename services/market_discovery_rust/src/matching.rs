use std::collections::HashMap;
use strsim::jaro_winkler;

/// Match confidence level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchConfidence {
    None = 0,
    Low = 1,      // Fuzzy match only - risky
    Medium = 2,   // Partial alias or word match
    High = 3,     // Strong alias match or multiple words
    Exact = 4,    // Normalized exact match
}

/// Result of matching two team names
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub confidence: MatchConfidence,
    pub score: f64,
    pub reason: String,
}

impl MatchResult {
    fn none() -> Self {
        Self {
            confidence: MatchConfidence::None,
            score: 0.0,
            reason: "No match".to_string(),
        }
    }

    fn exact() -> Self {
        Self {
            confidence: MatchConfidence::Exact,
            score: 1.0,
            reason: "Exact match".to_string(),
        }
    }

    fn high(score: f64, reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::High,
            score,
            reason: reason.to_string(),
        }
    }

    fn medium(score: f64, reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::Medium,
            score,
            reason: reason.to_string(),
        }
    }

    pub fn is_match(&self) -> bool {
        self.confidence >= MatchConfidence::Medium
    }
}

/// Get team aliases for a sport. Returns a map from canonical name -> list of aliases.
/// All names should be lowercase.
fn get_team_aliases(sport: &str) -> HashMap<&'static str, Vec<&'static str>> {
    let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::new();

    match sport {
        "nba" => {
            // Format: canonical_name -> [aliases including city, abbr, nicknames]
            map.insert("76ers", vec!["sixers", "philadelphia 76ers", "philadelphia sixers", "phi", "philly"]);
            map.insert("bucks", vec!["milwaukee bucks", "milwaukee", "mil"]);
            map.insert("bulls", vec!["chicago bulls", "chicago", "chi"]);
            map.insert("cavaliers", vec!["cavs", "cleveland cavaliers", "cleveland", "cle"]);
            map.insert("celtics", vec!["boston celtics", "boston", "bos"]);
            map.insert("clippers", vec!["la clippers", "los angeles clippers", "clips", "lac"]);
            map.insert("grizzlies", vec!["memphis grizzlies", "memphis", "mem"]);
            map.insert("hawks", vec!["atlanta hawks", "atlanta", "atl"]);
            map.insert("heat", vec!["miami heat", "miami", "mia"]);
            map.insert("hornets", vec!["charlotte hornets", "charlotte", "cha"]);
            map.insert("jazz", vec!["utah jazz", "utah", "uta"]);
            map.insert("kings", vec!["sacramento kings", "sacramento", "sac"]);
            map.insert("knicks", vec!["new york knicks", "ny knicks", "nyk"]);
            map.insert("lakers", vec!["la lakers", "los angeles lakers", "lal"]);
            map.insert("magic", vec!["orlando magic", "orlando", "orl"]);
            map.insert("mavericks", vec!["mavs", "dallas mavericks", "dallas", "dal"]);
            map.insert("nets", vec!["brooklyn nets", "brooklyn", "bkn"]);
            map.insert("nuggets", vec!["denver nuggets", "denver", "den"]);
            map.insert("pacers", vec!["indiana pacers", "indiana", "ind"]);
            map.insert("pelicans", vec!["pels", "new orleans pelicans", "new orleans", "nop"]);
            map.insert("pistons", vec!["detroit pistons", "detroit", "det"]);
            map.insert("raptors", vec!["toronto raptors", "toronto", "tor"]);
            map.insert("rockets", vec!["houston rockets", "houston", "hou"]);
            map.insert("spurs", vec!["san antonio spurs", "san antonio", "sas"]);
            map.insert("suns", vec!["phoenix suns", "phoenix", "phx"]);
            map.insert("thunder", vec!["oklahoma city thunder", "oklahoma city", "okc"]);
            map.insert("timberwolves", vec!["wolves", "minnesota timberwolves", "minnesota", "min"]);
            map.insert("trail blazers", vec!["blazers", "portland trail blazers", "portland", "por"]);
            map.insert("warriors", vec!["golden state warriors", "golden state", "gsw", "gs", "dubs"]);
            map.insert("wizards", vec!["washington wizards", "washington", "was"]);
        }
        "nfl" => {
            map.insert("49ers", vec!["niners", "san francisco 49ers", "san francisco", "sf"]);
            map.insert("bears", vec!["chicago bears", "chicago", "chi"]);
            map.insert("bengals", vec!["cincinnati bengals", "cincinnati", "cin"]);
            map.insert("bills", vec!["buffalo bills", "buffalo", "buf"]);
            map.insert("broncos", vec!["denver broncos", "denver", "den"]);
            map.insert("browns", vec!["cleveland browns", "cleveland", "cle"]);
            map.insert("buccaneers", vec!["bucs", "tampa bay buccaneers", "tampa bay", "tampa", "tb"]);
            map.insert("cardinals", vec!["cards", "arizona cardinals", "arizona", "ari"]);
            map.insert("chargers", vec!["la chargers", "los angeles chargers", "lac"]);
            map.insert("chiefs", vec!["kansas city chiefs", "kansas city", "kc"]);
            map.insert("colts", vec!["indianapolis colts", "indianapolis", "ind"]);
            map.insert("commanders", vec!["washington commanders", "washington", "was"]);
            map.insert("cowboys", vec!["dallas cowboys", "dallas", "dal"]);
            map.insert("dolphins", vec!["miami dolphins", "miami", "mia"]);
            map.insert("eagles", vec!["philadelphia eagles", "philadelphia", "phi", "philly"]);
            map.insert("falcons", vec!["atlanta falcons", "atlanta", "atl"]);
            map.insert("giants", vec!["new york giants", "ny giants", "nyg"]);
            map.insert("jaguars", vec!["jags", "jacksonville jaguars", "jacksonville", "jax"]);
            map.insert("jets", vec!["new york jets", "ny jets", "nyj"]);
            map.insert("lions", vec!["detroit lions", "detroit", "det"]);
            map.insert("packers", vec!["green bay packers", "green bay", "gb"]);
            map.insert("panthers", vec!["carolina panthers", "carolina", "car"]);
            map.insert("patriots", vec!["pats", "new england patriots", "new england", "ne"]);
            map.insert("raiders", vec!["las vegas raiders", "las vegas", "lv"]);
            map.insert("rams", vec!["la rams", "los angeles rams", "lar"]);
            map.insert("ravens", vec!["baltimore ravens", "baltimore", "bal"]);
            map.insert("saints", vec!["new orleans saints", "new orleans", "no"]);
            map.insert("seahawks", vec!["seattle seahawks", "seattle", "sea"]);
            map.insert("steelers", vec!["pittsburgh steelers", "pittsburgh", "pit"]);
            map.insert("texans", vec!["houston texans", "houston", "hou"]);
            map.insert("titans", vec!["tennessee titans", "tennessee", "ten"]);
            map.insert("vikings", vec!["minnesota vikings", "minnesota", "min"]);
        }
        "nhl" => {
            map.insert("avalanche", vec!["avs", "colorado avalanche", "colorado", "col"]);
            map.insert("blackhawks", vec!["chicago blackhawks", "chicago", "chi"]);
            map.insert("blue jackets", vec!["jackets", "columbus blue jackets", "columbus", "cbj"]);
            map.insert("blues", vec!["st louis blues", "st. louis blues", "stl"]);
            map.insert("bruins", vec!["boston bruins", "boston", "bos"]);
            map.insert("canadiens", vec!["habs", "montreal canadiens", "montreal", "mtl"]);
            map.insert("canucks", vec!["vancouver canucks", "vancouver", "van"]);
            map.insert("capitals", vec!["caps", "washington capitals", "washington", "wsh"]);
            map.insert("coyotes", vec!["yotes", "arizona coyotes", "arizona", "ari"]);
            map.insert("devils", vec!["new jersey devils", "new jersey", "nj", "njd"]);
            map.insert("ducks", vec!["anaheim ducks", "anaheim", "ana"]);
            map.insert("flames", vec!["calgary flames", "calgary", "cgy"]);
            map.insert("flyers", vec!["philadelphia flyers", "philadelphia", "phi"]);
            map.insert("golden knights", vec!["knights", "vegas golden knights", "vegas", "vgk"]);
            map.insert("hurricanes", vec!["canes", "carolina hurricanes", "carolina", "car"]);
            map.insert("islanders", vec!["isles", "new york islanders", "nyi"]);
            map.insert("jets", vec!["winnipeg jets", "winnipeg", "wpg"]);
            map.insert("kings", vec!["la kings", "los angeles kings", "lak"]);
            map.insert("kraken", vec!["seattle kraken", "seattle", "sea"]);
            map.insert("lightning", vec!["bolts", "tampa bay lightning", "tampa bay", "tbl"]);
            map.insert("maple leafs", vec!["leafs", "toronto maple leafs", "toronto", "tor"]);
            map.insert("oilers", vec!["edmonton oilers", "edmonton", "edm"]);
            map.insert("panthers", vec!["florida panthers", "florida", "fla"]);
            map.insert("penguins", vec!["pens", "pittsburgh penguins", "pittsburgh", "pit"]);
            map.insert("predators", vec!["preds", "nashville predators", "nashville", "nsh"]);
            map.insert("rangers", vec!["new york rangers", "nyr"]);
            map.insert("red wings", vec!["wings", "detroit red wings", "detroit", "det"]);
            map.insert("sabres", vec!["buffalo sabres", "buffalo", "buf"]);
            map.insert("senators", vec!["sens", "ottawa senators", "ottawa", "ott"]);
            map.insert("sharks", vec!["san jose sharks", "san jose", "sjs"]);
            map.insert("stars", vec!["dallas stars", "dallas", "dal"]);
            map.insert("wild", vec!["minnesota wild", "minnesota", "min"]);
        }
        "mlb" => {
            map.insert("angels", vec!["los angeles angels", "la angels", "anaheim angels", "laa"]);
            map.insert("astros", vec!["stros", "houston astros", "houston", "hou"]);
            map.insert("athletics", vec!["a's", "as", "oakland athletics", "oakland", "oak"]);
            map.insert("blue jays", vec!["jays", "toronto blue jays", "toronto", "tor"]);
            map.insert("braves", vec!["atlanta braves", "atlanta", "atl"]);
            map.insert("brewers", vec!["milwaukee brewers", "milwaukee", "mil"]);
            map.insert("cardinals", vec!["cards", "st louis cardinals", "st. louis cardinals", "stl"]);
            map.insert("cubs", vec!["chicago cubs", "chi cubs", "chc"]);
            map.insert("diamondbacks", vec!["dbacks", "d-backs", "arizona diamondbacks", "arizona", "ari"]);
            map.insert("dodgers", vec!["los angeles dodgers", "la dodgers", "lad"]);
            map.insert("giants", vec!["san francisco giants", "sf giants", "sf"]);
            map.insert("guardians", vec!["cleveland guardians", "cleveland", "cle"]);
            map.insert("mariners", vec!["seattle mariners", "seattle", "sea"]);
            map.insert("marlins", vec!["miami marlins", "florida marlins", "miami", "mia"]);
            map.insert("mets", vec!["new york mets", "ny mets", "nym"]);
            map.insert("nationals", vec!["nats", "washington nationals", "washington", "wsh"]);
            map.insert("orioles", vec!["o's", "os", "baltimore orioles", "baltimore", "bal"]);
            map.insert("padres", vec!["san diego padres", "san diego", "sd"]);
            map.insert("phillies", vec!["philadelphia phillies", "philadelphia", "phi", "philly"]);
            map.insert("pirates", vec!["pittsburgh pirates", "pittsburgh", "pit"]);
            map.insert("rangers", vec!["texas rangers", "texas", "tex"]);
            map.insert("rays", vec!["tampa bay rays", "tampa bay", "tb"]);
            map.insert("red sox", vec!["redsox", "boston red sox", "boston", "bos"]);
            map.insert("reds", vec!["cincinnati reds", "cincinnati", "cin"]);
            map.insert("rockies", vec!["colorado rockies", "colorado", "col"]);
            map.insert("royals", vec!["kansas city royals", "kansas city", "kc"]);
            map.insert("tigers", vec!["detroit tigers", "detroit", "det"]);
            map.insert("twins", vec!["minnesota twins", "minnesota", "min"]);
            map.insert("white sox", vec!["whitesox", "chicago white sox", "chi sox", "chw"]);
            map.insert("yankees", vec!["yanks", "new york yankees", "ny yankees", "nyy"]);
        }
        "ncaab" | "ncaaf" => {
            // College teams - include common nicknames and abbreviations
            map.insert("alabama", vec!["bama", "crimson tide", "ala"]);
            map.insert("arizona", vec!["wildcats", "ari", "zona"]);
            map.insert("arizona state", vec!["sun devils", "asu"]);
            map.insert("arkansas", vec!["razorbacks", "hogs", "ark"]);
            map.insert("auburn", vec!["tigers", "aub"]);
            map.insert("baylor", vec!["bears", "bay"]);
            map.insert("boston college", vec!["eagles", "bc"]);
            map.insert("clemson", vec!["tigers", "clem"]);
            map.insert("colorado", vec!["buffaloes", "buffs", "col", "cu"]);
            map.insert("connecticut", vec!["uconn", "huskies", "conn"]);
            map.insert("duke", vec!["blue devils"]);
            map.insert("florida", vec!["gators", "fla", "uf"]);
            map.insert("florida state", vec!["seminoles", "noles", "fsu"]);
            map.insert("georgia", vec!["bulldogs", "dawgs", "uga"]);
            map.insert("gonzaga", vec!["zags", "bulldogs"]);
            map.insert("houston", vec!["cougars", "coogs", "hou"]);
            map.insert("illinois", vec!["fighting illini", "illini", "ill"]);
            map.insert("indiana", vec!["hoosiers", "ind", "iu"]);
            map.insert("iowa", vec!["hawkeyes"]);
            map.insert("iowa state", vec!["cyclones", "isu"]);
            map.insert("kansas", vec!["jayhawks", "ku"]);
            map.insert("kansas state", vec!["wildcats", "ksu", "k-state"]);
            map.insert("kentucky", vec!["wildcats", "uk", "ky"]);
            map.insert("louisiana state", vec!["lsu", "tigers"]);
            map.insert("louisville", vec!["cardinals", "cards", "lou"]);
            map.insert("marquette", vec!["golden eagles", "marq"]);
            map.insert("maryland", vec!["terrapins", "terps", "md"]);
            map.insert("memphis", vec!["tigers", "mem"]);
            map.insert("miami", vec!["hurricanes", "canes", "mia"]);
            map.insert("michigan", vec!["wolverines", "mich", "um"]);
            map.insert("michigan state", vec!["spartans", "msu"]);
            map.insert("minnesota", vec!["golden gophers", "gophers", "minn"]);
            map.insert("mississippi", vec!["ole miss", "rebels"]);
            map.insert("mississippi state", vec!["bulldogs", "miss st", "msst"]);
            map.insert("missouri", vec!["tigers", "mizzou", "miz"]);
            map.insert("north carolina", vec!["tar heels", "unc", "carolina"]);
            map.insert("north carolina state", vec!["wolfpack", "nc state", "ncsu"]);
            map.insert("notre dame", vec!["fighting irish", "irish", "nd"]);
            map.insert("ohio state", vec!["buckeyes", "osu"]);
            map.insert("oklahoma", vec!["sooners", "ou"]);
            map.insert("oklahoma state", vec!["cowboys", "okst"]);
            map.insert("oregon", vec!["ducks", "ore"]);
            map.insert("oregon state", vec!["beavers", "orst"]);
            map.insert("penn state", vec!["nittany lions", "psu"]);
            map.insert("pittsburgh", vec!["panthers", "pitt"]);
            map.insert("purdue", vec!["boilermakers", "pur"]);
            map.insert("rutgers", vec!["scarlet knights", "rut"]);
            map.insert("san diego state", vec!["aztecs", "sdsu"]);
            map.insert("south carolina", vec!["gamecocks", "scar"]);
            map.insert("stanford", vec!["cardinal", "stan"]);
            map.insert("syracuse", vec!["orange", "cuse"]);
            map.insert("tcu", vec!["horned frogs"]);
            map.insert("tennessee", vec!["volunteers", "vols", "tenn"]);
            map.insert("texas", vec!["longhorns", "horns", "tex", "ut"]);
            map.insert("texas a&m", vec!["aggies", "tamu"]);
            map.insert("texas tech", vec!["red raiders", "ttu"]);
            map.insert("ucla", vec!["bruins"]);
            map.insert("usc", vec!["trojans", "southern cal", "southern california"]);
            map.insert("utah", vec!["utes"]);
            map.insert("vanderbilt", vec!["commodores", "vandy"]);
            map.insert("villanova", vec!["wildcats", "nova"]);
            map.insert("virginia", vec!["cavaliers", "cavs", "uva", "hoos"]);
            map.insert("virginia tech", vec!["hokies", "vt"]);
            map.insert("wake forest", vec!["demon deacons", "wake"]);
            map.insert("washington", vec!["huskies", "udub", "uw"]);
            map.insert("west virginia", vec!["mountaineers", "wvu"]);
            map.insert("wisconsin", vec!["badgers", "wisc"]);
            map.insert("xavier", vec!["musketeers"]);
        }
        "soccer" => {
            // Premier League
            map.insert("arsenal", vec!["gunners", "ars"]);
            map.insert("aston villa", vec!["villa", "avl"]);
            map.insert("bournemouth", vec!["cherries", "bou"]);
            map.insert("brentford", vec!["bees", "bre"]);
            map.insert("brighton", vec!["seagulls", "brighton and hove albion", "bha"]);
            map.insert("chelsea", vec!["blues", "che"]);
            map.insert("crystal palace", vec!["palace", "eagles", "cry"]);
            map.insert("everton", vec!["toffees", "eve"]);
            map.insert("fulham", vec!["cottagers", "ful"]);
            map.insert("ipswich", vec!["tractor boys", "ipswich town", "ips"]);
            map.insert("leicester", vec!["foxes", "leicester city", "lei"]);
            map.insert("liverpool", vec!["reds", "liv"]);
            map.insert("manchester city", vec!["man city", "city", "mci"]);
            map.insert("manchester united", vec!["man utd", "man united", "united", "mun"]);
            map.insert("newcastle", vec!["magpies", "newcastle united", "new"]);
            map.insert("nottingham forest", vec!["forest", "nfo"]);
            map.insert("southampton", vec!["saints", "sou"]);
            map.insert("tottenham", vec!["spurs", "tottenham hotspur", "tot"]);
            map.insert("west ham", vec!["hammers", "west ham united", "whu"]);
            map.insert("wolves", vec!["wolverhampton", "wolverhampton wanderers", "wol"]);
            // Top European clubs
            map.insert("barcelona", vec!["barca", "fcb"]);
            map.insert("real madrid", vec!["real", "madrid", "rma"]);
            map.insert("atletico madrid", vec!["atletico", "atm"]);
            map.insert("bayern munich", vec!["bayern", "fcb"]);
            map.insert("borussia dortmund", vec!["dortmund", "bvb"]);
            map.insert("paris saint germain", vec!["psg", "paris"]);
            map.insert("juventus", vec!["juve"]);
            map.insert("inter milan", vec!["inter", "internazionale"]);
            map.insert("ac milan", vec!["milan"]);
            map.insert("napoli", vec!["nap"]);
        }
        "mma" | "ufc" => {
            // MMA typically uses fighter names, not team names
            // Add common name variations if needed
        }
        "tennis" => {
            // Tennis uses player names
        }
        _ => {}
    }

    map
}

/// Words that are too generic to be reliable team identifiers
const GENERIC_WORDS: &[&str] = &[
    "state", "city", "university", "college", "fc", "united", "team",
    "the", "of", "and", "vs", "at", "in", "to", "for",
    "los", "san", "new", "las", // City prefixes
];

/// Words that indicate non-moneyline market types
const NON_MONEYLINE_INDICATORS: &[&str] = &[
    "over", "under", "o/u", "total", "spread", "handicap",
    "points", "goals", "runs", "score", "combined",
    "first to", "most", "mvp", "player", "quarter", "half",
    "period", "inning", "how many", "exact", "margin",
];

/// Normalize a string for comparison
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Tokenize into words
fn tokenize(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .map(|w| w.to_string())
        .collect()
}

/// Check if text contains phrase as whole words (not substring of another word)
fn contains_phrase(text: &str, phrase: &str) -> bool {
    let text_words: Vec<&str> = text.split_whitespace().collect();
    let phrase_words: Vec<&str> = phrase.split_whitespace().collect();

    if phrase_words.is_empty() {
        return false;
    }

    // Look for contiguous phrase match
    if phrase_words.len() > 1 {
        for window in text_words.windows(phrase_words.len()) {
            if window == phrase_words.as_slice() {
                return true;
            }
        }
        return false;
    }

    // For single word phrases, check if it's a complete word in the text
    text_words.contains(&phrase_words[0])
}

/// Check if target matches candidate using alias lookup
fn check_alias_match(target_norm: &str, candidate_norm: &str, aliases: &HashMap<&str, Vec<&str>>) -> Option<(f64, String)> {
    // Direct lookup: is target a canonical name?
    if let Some(alias_list) = aliases.get(target_norm) {
        // Check if candidate matches any alias
        for alias in alias_list {
            let alias_norm = normalize(alias);
            if candidate_norm == alias_norm {
                return Some((0.95, format!("Alias exact: {} -> {}", target_norm, alias)));
            }
            if contains_phrase(candidate_norm, &alias_norm) {
                return Some((0.9, format!("Alias in text: {} -> {}", target_norm, alias)));
            }
        }
        // Also check if candidate contains the canonical name
        if contains_phrase(candidate_norm, target_norm) {
            return Some((0.9, format!("Canonical in text: {}", target_norm)));
        }
    }

    // Reverse lookup: is target an alias for some canonical name?
    for (canonical, alias_list) in aliases.iter() {
        let canonical_norm = normalize(canonical);

        // Check if target matches this canonical or any of its aliases
        let target_matches_group = target_norm == canonical_norm
            || alias_list.iter().any(|a| normalize(a) == target_norm);

        if target_matches_group {
            // Now check if candidate matches canonical
            if candidate_norm == canonical_norm {
                return Some((0.95, format!("Reverse: {} -> canonical {}", target_norm, canonical)));
            }
            if contains_phrase(candidate_norm, &canonical_norm) {
                return Some((0.9, format!("Reverse canonical in text: {} -> {}", target_norm, canonical)));
            }

            // Check if candidate matches any alias in this group
            for alias in alias_list {
                let alias_norm = normalize(alias);
                if candidate_norm == alias_norm {
                    return Some((0.95, format!("Reverse alias: {} -> {}", target_norm, alias)));
                }
                if contains_phrase(candidate_norm, &alias_norm) {
                    return Some((0.9, format!("Reverse alias in text: {} -> {}", target_norm, alias)));
                }
            }
        }
    }

    None
}

/// Check if a word is too generic to be a reliable identifier
fn is_generic_word(word: &str) -> bool {
    GENERIC_WORDS.contains(&word)
}

/// Match a team name against text (like a market question).
/// Returns a MatchResult with confidence level and score.
pub fn match_team_in_text(team_name: &str, text: &str, sport: &str) -> MatchResult {
    let t_norm = normalize(team_name);
    let text_norm = normalize(text);

    if t_norm.is_empty() || text_norm.is_empty() {
        return MatchResult::none();
    }

    // Pre-check: tokenize and filter generic words early
    let t_words = tokenize(team_name);
    let text_words = tokenize(text);

    // Filter out generic words from target for matching purposes
    let t_significant: Vec<&String> = t_words.iter()
        .filter(|w| !is_generic_word(w))
        .collect();

    // If the team name consists only of generic words, don't match
    if t_significant.is_empty() {
        return MatchResult::none();
    }

    // 1. Exact phrase match (highest confidence)
    // Only check if team name has at least one significant word
    if contains_phrase(&text_norm, &t_norm) {
        return MatchResult::exact();
    }

    // 2. Alias-based matching (sport-specific)
    let aliases = get_team_aliases(sport);
    if let Some((score, reason)) = check_alias_match(&t_norm, &text_norm, &aliases) {
        return MatchResult::high(score, &reason);
    }

    // For team names with mascot (e.g., "Cleveland Cavaliers"):
    // - Prefer matching the LAST significant word (usually the mascot)
    // - Mascots are more unique than city names
    if t_significant.len() >= 1 {
        // Get the last significant word (likely the mascot/nickname)
        if let Some(mascot) = t_significant.last() {
            // Check it's not a common mascot shared across many teams
            let shared_mascots = ["tigers", "wildcats", "bulldogs", "eagles", "cardinals", "panthers"];

            if !shared_mascots.contains(&mascot.as_str()) {
                if text_words.contains(mascot) {
                    return MatchResult::high(0.85, &format!("Mascot match: {}", mascot));
                }
            } else if t_significant.len() >= 2 {
                // For shared mascots, require at least one more significant word to match
                let other_matches: usize = t_significant.iter()
                    .filter(|w| *w != mascot && text_words.contains(*w))
                    .count();
                if other_matches >= 1 && text_words.contains(mascot) {
                    return MatchResult::high(0.85, &format!("Team + mascot: {} + {}",
                        t_significant.iter().find(|w| *w != mascot && text_words.contains(*w)).map(|s| s.as_str()).unwrap_or(""),
                        mascot));
                }
            }
        }
    }

    // For single-word team names (e.g., "Lakers", "Celtics")
    if t_significant.len() == 1 {
        let word = &t_significant[0];
        if text_words.contains(word) {
            return MatchResult::high(0.85, &format!("Single word: {}", word));
        }
    }

    // Multi-word overlap: require strong overlap
    // Must match the majority of significant words, with at least 2 matches
    let match_count: usize = t_significant.iter()
        .filter(|w| text_words.contains(*w))
        .count();

    let overlap_ratio = match_count as f64 / t_significant.len() as f64;

    if match_count >= 2 && overlap_ratio >= 0.5 {
        return MatchResult::medium(overlap_ratio, &format!("Word overlap: {}/{}", match_count, t_significant.len()));
    }

    // 4. Fuzzy match - ONLY for single words that are reasonably long
    // Using VERY high threshold to minimize false positives
    if t_significant.len() == 1 && t_norm.len() >= 6 {
        // Try to fuzzy match the single significant word against text words
        let word = &t_significant[0];
        for tw in &text_words {
            if tw.len() >= 5 {
                let score = jaro_winkler(word, tw);
                if score > 0.95 {  // Very high threshold
                    return MatchResult::medium(score, &format!("Fuzzy: {} ~ {}", word, tw));
                }
            }
        }
    }

    MatchResult::none()
}

/// Legacy API: Check if two team names match (for backwards compatibility).
pub fn names_match(target: &str, candidate: &str, sport: &str) -> bool {
    match_team_in_text(target, candidate, sport).is_match()
}

/// Match a game (home + away team) against market text.
/// Returns (is_match, home_result, away_result).
/// Only returns true if BOTH teams are found with sufficient confidence.
pub fn match_game_in_text(
    home_team: &str,
    away_team: &str,
    home_abbr: &str,
    away_abbr: &str,
    text: &str,
    sport: &str,
) -> (bool, MatchResult, MatchResult) {
    // Try to match home team (full name or abbreviation)
    let home_result = match_team_in_text(home_team, text, sport);
    let home_abbr_result = match_team_in_text(home_abbr, text, sport);
    let best_home = if home_result.score >= home_abbr_result.score {
        home_result
    } else {
        home_abbr_result
    };

    // Try to match away team (full name or abbreviation)
    let away_result = match_team_in_text(away_team, text, sport);
    let away_abbr_result = match_team_in_text(away_abbr, text, sport);
    let best_away = if away_result.score >= away_abbr_result.score {
        away_result
    } else {
        away_abbr_result
    };

    // Both teams must match for a valid game match
    let is_match = best_home.is_match() && best_away.is_match();

    (is_match, best_home, best_away)
}

/// Check if a market question indicates a non-moneyline market.
pub fn is_non_moneyline_market(question: &str) -> bool {
    let q = question.to_lowercase();

    // Check for non-moneyline indicators
    for indicator in NON_MONEYLINE_INDICATORS {
        if q.contains(indicator) {
            return true;
        }
    }

    // Check for spread patterns like "+5.5" or "-3.5"
    // Look for +/- followed by a digit (but not in a time like "7:30")
    let chars: Vec<char> = q.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if (c == '+' || c == '-') && i > 0 {
            // Check if preceded by a space (not part of a word or time)
            if chars[i - 1] == ' ' {
                // Check if followed by a digit
                if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== TRUE POSITIVES (should match) ==========

    #[test]
    fn test_exact_match() {
        assert!(names_match("Lakers", "Los Angeles Lakers", "nba"));
        assert!(names_match("Los Angeles Lakers", "Lakers", "nba"));
    }

    #[test]
    fn test_alias_match() {
        assert!(names_match("Sixers", "76ers", "nba"));
        assert!(names_match("76ers", "Philadelphia 76ers", "nba"));
        assert!(names_match("Cavs", "Cleveland Cavaliers", "nba"));
        assert!(names_match("Mavs", "Dallas Mavericks", "nba"));
    }

    #[test]
    fn test_city_plus_team() {
        assert!(names_match("Philadelphia 76ers", "76ers", "nba"));
        assert!(names_match("Golden State Warriors", "Warriors", "nba"));
    }

    #[test]
    fn test_abbreviation_match() {
        assert!(names_match("LAL", "Los Angeles Lakers", "nba"));
        assert!(names_match("GSW", "Golden State Warriors", "nba"));
        assert!(names_match("BOS", "Boston Celtics", "nba"));
    }

    #[test]
    fn test_college_aliases() {
        assert!(names_match("UConn", "Connecticut", "ncaab"));
        assert!(names_match("Bama", "Alabama", "ncaaf"));
        assert!(names_match("Notre Dame", "Notre Dame Fighting Irish", "ncaaf"));
        assert!(names_match("LSU", "Louisiana State", "ncaab"));
    }

    #[test]
    fn test_soccer_aliases() {
        assert!(names_match("Man City", "Manchester City", "soccer"));
        assert!(names_match("Spurs", "Tottenham", "soccer"));
        assert!(names_match("Villa", "Aston Villa", "soccer"));
    }

    #[test]
    fn test_nhl_aliases() {
        assert!(names_match("Caps", "Washington Capitals", "nhl"));
        assert!(names_match("Pens", "Pittsburgh Penguins", "nhl"));
        assert!(names_match("Leafs", "Toronto Maple Leafs", "nhl"));
    }

    #[test]
    fn test_nhl_game_match_moneyline_short_question() {
        // Polymarket often omits city names in NHL moneyline questions.
        // Example from Gamma: "Golden Knights vs. Maple Leafs"
        let (is_match, home, away) = match_game_in_text(
            "Toronto Maple Leafs",
            "Vegas Golden Knights",
            "TOR",
            "VGK",
            "Golden Knights vs. Maple Leafs",
            "nhl",
        );
        assert!(is_match, "Home: {:?}, Away: {:?}", home, away);
    }

    #[test]
    fn test_nfl_aliases() {
        assert!(names_match("Niners", "San Francisco 49ers", "nfl"));
        assert!(names_match("49ers", "San Francisco 49ers", "nfl"));
        assert!(names_match("Pats", "New England Patriots", "nfl"));
    }

    #[test]
    fn test_mlb_aliases() {
        assert!(names_match("Yanks", "New York Yankees", "mlb"));
        assert!(names_match("Cards", "St Louis Cardinals", "mlb"));
    }

    #[test]
    fn test_market_question_matching() {
        let result = match_team_in_text(
            "Lakers",
            "Will the Los Angeles Lakers beat the Boston Celtics?",
            "nba",
        );
        assert!(result.is_match());

        let result = match_team_in_text(
            "Celtics",
            "Will the Los Angeles Lakers beat the Boston Celtics?",
            "nba",
        );
        assert!(result.is_match());
    }

    #[test]
    fn test_full_game_match() {
        let (is_match, home, away) = match_game_in_text(
            "Los Angeles Lakers",
            "Boston Celtics",
            "LAL",
            "BOS",
            "Lakers vs Celtics: Who will win?",
            "nba",
        );
        assert!(is_match, "Home: {:?}, Away: {:?}", home, away);
    }

    // ========== TRUE NEGATIVES (should NOT match) ==========

    #[test]
    fn test_no_false_positive_similar_city() {
        // "Cleveland Cavaliers" should NOT match "Cleveland State"
        let result = match_team_in_text("Cleveland Cavaliers", "Cleveland State vs Duke", "ncaab");
        assert!(!result.is_match(), "Cavaliers should not match Cleveland State");
    }

    #[test]
    fn test_no_cross_sport_false_positive() {
        // Hawks (NBA Atlanta) should not match Blackhawks (NHL Chicago)
        // because sport-specific aliases are used
        let result = match_team_in_text("Hawks", "Chicago Blackhawks game tonight", "nba");
        assert!(!result.is_match(), "NBA Hawks should not match text about Blackhawks");
    }

    #[test]
    fn test_no_generic_word_match() {
        // "State" alone should not match
        let result = match_team_in_text("State", "Ohio State vs Michigan", "ncaaf");
        assert!(!result.is_match(), "Generic word 'State' should not match");
    }

    #[test]
    fn test_no_fuzzy_false_positive() {
        // Similar but different teams should not match via fuzzy
        let result = match_team_in_text("Warriors", "Washington Wizards game", "nba");
        assert!(!result.is_match(), "Warriors should not fuzzy-match Wizards");
    }

    #[test]
    fn test_different_teams_same_city() {
        // Lakers should not match text about Clippers
        let result = match_team_in_text("Los Angeles Lakers", "LA Clippers vs Denver Nuggets", "nba");
        assert!(!result.is_match(), "Lakers should not match Clippers");
    }

    // ========== NON-MONEYLINE DETECTION ==========

    #[test]
    fn test_non_moneyline_detection() {
        assert!(is_non_moneyline_market("Lakers vs Celtics O/U 220.5"));
        assert!(is_non_moneyline_market("Lakers +5.5 spread"));
        assert!(is_non_moneyline_market("Total points over 200"));
        assert!(is_non_moneyline_market("First to score 20 points"));
        assert!(is_non_moneyline_market("Lebron MVP odds"));

        // These should NOT be flagged as non-moneyline
        assert!(!is_non_moneyline_market("Will Lakers beat Celtics?"));
        assert!(!is_non_moneyline_market("Lakers vs Celtics winner"));
    }

    // ========== EDGE CASES ==========

    #[test]
    fn test_case_insensitive() {
        assert!(names_match("LAKERS", "lakers", "nba"));
        assert!(names_match("Lakers", "LAKERS", "nba"));
    }

    #[test]
    fn test_punctuation_handling() {
        assert!(names_match("76ers", "76ers", "nba"));
    }

    #[test]
    fn test_empty_strings() {
        assert!(!names_match("", "Lakers", "nba"));
        assert!(!names_match("Lakers", "", "nba"));
        assert!(!names_match("", "", "nba"));
    }

    #[test]
    fn test_numeric_teams() {
        assert!(names_match("76ers", "Philadelphia 76ers", "nba"));
        assert!(names_match("49ers", "San Francisco 49ers", "nfl"));
    }

    // ========== SHARED MASCOT HANDLING ==========

    #[test]
    fn test_shared_mascot_requires_context() {
        // "Tigers" is used by multiple teams - should require more context
        let result = match_team_in_text("Tigers", "Detroit Tigers vs Yankees", "mlb");
        // Single word "Tigers" should match since it appears in text
        assert!(result.is_match());

        // But "Auburn Tigers" should only match Auburn-specific text
        let result = match_team_in_text("Auburn Tigers", "Clemson Tigers vs Duke", "ncaaf");
        assert!(!result.is_match(), "Auburn should not match Clemson");
    }
}
